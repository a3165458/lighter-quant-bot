use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, info};

use crate::lighter::types::*;
use crate::strategy::Strategy;

/// 回测交易记录
#[derive(Debug, Clone)]
pub struct BacktestTrade {
    pub timestamp: DateTime<Utc>,
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub quantity: f64,
    pub pnl: f64,
    pub commission: f64,
}

/// 回测结果
#[derive(Debug, Clone)]
pub struct BacktestResults {
    pub total_return: f64,
    pub sharpe_ratio: f64,
    pub max_drawdown: f64,
    pub win_rate: f64,
    pub trades: Vec<BacktestTrade>,
    pub equity_curve: Vec<(DateTime<Utc>, f64)>,
    pub initial_capital: f64,
    pub final_capital: f64,
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub avg_profit: f64,
    pub avg_loss: f64,
    pub profit_factor: f64,
}

/// 回测引擎
pub struct BacktestEngine {
    initial_capital: f64,
    capital: f64,
    historical_data: Vec<Candlestick>,
    commission_rate: f64,
    slippage_rate: f64,
    trades: Vec<BacktestTrade>,
    equity_curve: Vec<(DateTime<Utc>, f64)>,
}

impl BacktestEngine {
    pub fn new(initial_capital: f64, historical_data: Vec<Candlestick>) -> Self {
        Self {
            initial_capital,
            capital: initial_capital,
            historical_data,
            commission_rate: 0.001, // 0.1%
            slippage_rate: 0.0005,  // 0.05%
            trades: Vec::new(),
            equity_curve: Vec::new(),
        }
    }

    /// 设置手续费率
    #[allow(dead_code)]
    pub fn with_commission(mut self, rate: f64) -> Self {
        self.commission_rate = rate;
        self
    }

    /// 设置滑点
    #[allow(dead_code)]
    pub fn with_slippage(mut self, rate: f64) -> Self {
        self.slippage_rate = rate;
        self
    }

    /// 运行回测
    pub async fn run(&mut self, strategy: Box<dyn Strategy>) -> Result<BacktestResults> {
        info!("开始回测，数据量: {} 根K线", self.historical_data.len());

        let data = self.historical_data.clone();
        let mut position: Option<(Side, f64, f64)> = None; // (side, entry_price, quantity)

        // 预分配容量提升性能
        self.equity_curve.reserve(data.len());
        self.trades.reserve(data.len() / 10);

        for (i, candle) in data.iter().enumerate() {
            // 只传递最近的窗口数据构建快照，避免 O(n²) 克隆
            // 需要至少2根K线让策略比较前后价格
            let window_start = if i >= 1 { i.saturating_sub(100) } else { 0 };
            let snapshot = self.build_snapshot(&data[window_start..=i]);

            // 评估策略
            if let Some(signals) = strategy.evaluate(&snapshot).await? {
                for signal in signals {
                    // 模拟执行
                    let execution_price = self.apply_slippage(signal.price, signal.side);
                    let commission_per_qty = execution_price * self.commission_rate;

                    match position {
                        Some((pos_side, entry_price, pos_qty)) if pos_side == signal.side => {
                            let add_qty = signal.quantity;
                            let add_commission = commission_per_qty * add_qty;
                            let cost = execution_price * add_qty;

                            if cost + add_commission <= self.capital {
                                let new_qty = pos_qty + add_qty;
                                let weighted_entry =
                                    ((entry_price * pos_qty) + (execution_price * add_qty)) / new_qty;
                                self.capital -= add_commission;
                                position = Some((pos_side, weighted_entry, new_qty));
                                debug!(
                                    "加仓: {:?} {} @ {:.2}, qty {:.6} -> {:.6}",
                                    signal.side,
                                    signal.symbol,
                                    execution_price,
                                    pos_qty,
                                    new_qty
                                );
                            }
                        }
                        Some((pos_side, entry_price, pos_qty)) => {
                            let close_qty = pos_qty.min(signal.quantity);
                            let close_commission = commission_per_qty * close_qty;
                            let pnl = match pos_side {
                                Side::Buy => (execution_price - entry_price) * close_qty,
                                Side::Sell => (entry_price - execution_price) * close_qty,
                            };

                            self.capital += pnl - close_commission;
                            self.trades.push(BacktestTrade {
                                timestamp: candle.timestamp,
                                symbol: signal.symbol.clone(),
                                side: signal.side,
                                price: execution_price,
                                quantity: close_qty,
                                pnl,
                                commission: close_commission,
                            });

                            debug!("平仓: {} @ {:.2}, PnL: {:.2}", signal.symbol, execution_price, pnl);

                            let remaining_pos_qty = pos_qty - close_qty;
                            let remaining_signal_qty = signal.quantity - close_qty;

                            position = if remaining_pos_qty > f64::EPSILON {
                                Some((pos_side, entry_price, remaining_pos_qty))
                            } else {
                                None
                            };

                            if remaining_signal_qty > f64::EPSILON {
                                let open_commission = commission_per_qty * remaining_signal_qty;
                                let cost = execution_price * remaining_signal_qty;
                                if cost + open_commission <= self.capital {
                                    self.capital -= open_commission;
                                    position = Some((signal.side, execution_price, remaining_signal_qty));
                                    debug!(
                                        "反手开仓: {:?} {} @ {:.2}, qty {:.6}",
                                        signal.side,
                                        signal.symbol,
                                        execution_price,
                                        remaining_signal_qty
                                    );
                                }
                            }
                        }
                        None => {
                            let commission = commission_per_qty * signal.quantity;
                            let cost = execution_price * signal.quantity;
                            if cost + commission <= self.capital {
                                self.capital -= commission;
                                position = Some((signal.side, execution_price, signal.quantity));
                                debug!("开仓: {:?} {} @ {:.2}", signal.side, signal.symbol, execution_price);
                            }
                        }
                    }
                }
            }

            // 记录权益曲线
            let unrealized_pnl = if let Some((side, entry, qty)) = position {
                match side {
                    Side::Buy => (candle.close - entry) * qty,
                    Side::Sell => (entry - candle.close) * qty,
                }
            } else {
                0.0
            };

            self.equity_curve.push((candle.timestamp, self.capital + unrealized_pnl));
        }

        // 强制平仓
        if let Some((side, entry_price, qty)) = position {
            if let Some(last_candle) = data.last() {
                let pnl = match side {
                    Side::Buy => (last_candle.close - entry_price) * qty,
                    Side::Sell => (entry_price - last_candle.close) * qty,
                };
                self.capital += pnl;
                self.trades.push(BacktestTrade {
                    timestamp: last_candle.timestamp,
                    symbol: last_candle.symbol.clone(),
                    side: if side == Side::Buy { Side::Sell } else { Side::Buy },
                    price: last_candle.close,
                    quantity: qty,
                    pnl,
                    commission: 0.0,
                });
            }
        }

        Ok(self.calculate_results())
    }

    /// 构建市场快照
    fn build_snapshot(&self, candles: &[Candlestick]) -> MarketSnapshot {
        let mut snapshot = MarketSnapshot::default();

        if let Some(last) = candles.last() {
            let ob = OrderBook {
                symbol: last.symbol.clone(),
                market_id: 0,
                bids: vec![PriceLevel { price: last.close * 0.999, quantity: 1.0 }],
                asks: vec![PriceLevel { price: last.close * 1.001, quantity: 1.0 }],
                timestamp: last.timestamp,
            };
            snapshot.order_books.insert(last.symbol.clone(), ob);

            let candle_vec: Vec<Candlestick> = candles.to_vec();
            snapshot.candles.insert(last.symbol.clone(), candle_vec);
        }

        snapshot
    }

    /// 应用滑点
    fn apply_slippage(&self, price: f64, side: Side) -> f64 {
        match side {
            Side::Buy => price * (1.0 + self.slippage_rate),
            Side::Sell => price * (1.0 - self.slippage_rate),
        }
    }

    /// 计算回测结果
    fn calculate_results(&self) -> BacktestResults {
        let total_return = (self.capital - self.initial_capital) / self.initial_capital;

        let winning_trades: Vec<&BacktestTrade> = self.trades.iter().filter(|t| t.pnl > 0.0).collect();
        let losing_trades: Vec<&BacktestTrade> = self.trades.iter().filter(|t| t.pnl <= 0.0).collect();

        let win_rate = if self.trades.is_empty() {
            0.0
        } else {
            winning_trades.len() as f64 / self.trades.len() as f64
        };

        let avg_profit = if winning_trades.is_empty() {
            0.0
        } else {
            winning_trades.iter().map(|t| t.pnl).sum::<f64>() / winning_trades.len() as f64
        };

        let avg_loss = if losing_trades.is_empty() {
            0.0
        } else {
            losing_trades.iter().map(|t| t.pnl).sum::<f64>() / losing_trades.len() as f64
        };

        let total_profit: f64 = winning_trades.iter().map(|t| t.pnl).sum();
        let total_loss: f64 = losing_trades.iter().map(|t| t.pnl.abs()).sum();
        let profit_factor = if total_loss > 0.0 { total_profit / total_loss } else { f64::INFINITY };

        // 计算最大回撤
        let max_drawdown = self.calculate_max_drawdown();

        // 计算夏普比率
        let sharpe_ratio = self.calculate_sharpe_ratio();

        BacktestResults {
            total_return,
            sharpe_ratio,
            max_drawdown,
            win_rate,
            trades: self.trades.clone(),
            equity_curve: self.equity_curve.clone(),
            initial_capital: self.initial_capital,
            final_capital: self.capital,
            total_trades: self.trades.len(),
            winning_trades: winning_trades.len(),
            losing_trades: losing_trades.len(),
            avg_profit,
            avg_loss,
            profit_factor,
        }
    }

    /// 计算最大回撤
    fn calculate_max_drawdown(&self) -> f64 {
        let mut max_equity = 0.0_f64;
        let mut max_drawdown = 0.0_f64;

        for (_, equity) in &self.equity_curve {
            max_equity = max_equity.max(*equity);
            let drawdown = (max_equity - equity) / max_equity;
            max_drawdown = max_drawdown.max(drawdown);
        }

        max_drawdown
    }

    /// 计算夏普比率（年化）
    fn calculate_sharpe_ratio(&self) -> f64 {
        if self.equity_curve.len() < 2 {
            return 0.0;
        }

        let returns: Vec<f64> = self.equity_curve
            .windows(2)
            .map(|w| (w[1].1 - w[0].1) / w[0].1)
            .collect();

        if returns.is_empty() {
            return 0.0;
        }

        let mean_return: f64 = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance: f64 = returns.iter()
            .map(|r| (r - mean_return).powi(2))
            .sum::<f64>() / returns.len() as f64;
        let std_dev = variance.sqrt();

        if std_dev == 0.0 {
            return 0.0;
        }

        // 按数据实际间隔推断年化因子（每年周期数 = 一年秒数 / 平均K线间隔秒数）
        let first_ts = self.equity_curve.first().unwrap().0;
        let last_ts = self.equity_curve.last().unwrap().0;
        let span_secs = (last_ts - first_ts).num_seconds().max(1) as f64;
        let avg_interval_secs = span_secs / (self.equity_curve.len() - 1) as f64;
        let periods_per_year = (365.25 * 86400.0) / avg_interval_secs.max(1.0);
        let annualized_factor = periods_per_year.sqrt();
        (mean_return / std_dev) * annualized_factor
    }
}
