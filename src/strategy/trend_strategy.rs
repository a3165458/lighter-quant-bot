use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use tracing::debug;

use crate::lighter::types::*;
use super::Strategy;

/// 趋势跟踪策略
///
/// 使用双均线交叉来判断趋势，结合止损止盈进行仓位管理
pub struct TrendStrategy {
    fast_ma_period: usize,
    slow_ma_period: usize,
    stop_loss_pct: f64,
    take_profit_pct: f64,
    #[allow(dead_code)]
    price_history: Vec<f64>,
    position_entry: Option<f64>,
    position_side: Option<Side>,
}

impl TrendStrategy {
    pub fn new(
        fast_ma_period: usize,
        slow_ma_period: usize,
        stop_loss_pct: f64,
        take_profit_pct: f64,
    ) -> Self {
        Self {
            fast_ma_period,
            slow_ma_period,
            stop_loss_pct,
            take_profit_pct,
            price_history: Vec::new(),
            position_entry: None,
            position_side: None,
        }
    }

    /// 计算简单移动平均线
    fn sma(prices: &[f64], period: usize) -> Option<f64> {
        if prices.len() < period {
            return None;
        }
        let sum: f64 = prices[prices.len() - period..].iter().sum();
        Some(sum / period as f64)
    }

    /// 检查止损止盈
    #[allow(dead_code)]
    fn check_exit(&self, current_price: f64) -> Option<String> {
        if let (Some(entry), Some(side)) = (self.position_entry, self.position_side) {
            let pnl_pct = match side {
                Side::Buy => (current_price - entry) / entry,
                Side::Sell => (entry - current_price) / entry,
            };

            if pnl_pct <= -self.stop_loss_pct {
                return Some(format!("止损触发: PnL {:.2}%", pnl_pct * 100.0));
            }
            if pnl_pct >= self.take_profit_pct {
                return Some(format!("止盈触发: PnL {:.2}%", pnl_pct * 100.0));
            }
        }
        None
    }

    /// 判断趋势方向
    #[allow(dead_code)]
    fn get_trend_signal(&self) -> Option<Side> {
        let fast_ma = Self::sma(&self.price_history, self.fast_ma_period)?;
        let slow_ma = Self::sma(&self.price_history, self.slow_ma_period)?;

        // 检查前一根的MA值
        if self.price_history.len() < self.slow_ma_period + 1 {
            return None;
        }

        let prev_prices = &self.price_history[..self.price_history.len() - 1];
        let prev_fast = Self::sma(prev_prices, self.fast_ma_period)?;
        let prev_slow = Self::sma(prev_prices, self.slow_ma_period)?;

        // 金叉：快线上穿慢线
        if prev_fast <= prev_slow && fast_ma > slow_ma {
            return Some(Side::Buy);
        }

        // 死叉：快线下穿慢线
        if prev_fast >= prev_slow && fast_ma < slow_ma {
            return Some(Side::Sell);
        }

        None
    }
}

#[async_trait]
impl Strategy for TrendStrategy {
    fn name(&self) -> &str {
        "trend_following"
    }

    async fn evaluate(&self, snapshot: &MarketSnapshot) -> Result<Option<Vec<TradeSignal>>> {
        let mut signals = Vec::new();

        for (symbol, ob) in &snapshot.order_books {
            if let Some(mid_price) = ob.mid_price() {
                // 从快照中取价格历史
                let candles = snapshot.candles.get(symbol);
                let prices: Vec<f64> = if let Some(candles) = candles {
                    candles.iter().map(|c| c.close).collect()
                } else {
                    vec![mid_price]
                };

                if prices.len() < self.slow_ma_period + 1 {
                    debug!("{}: 价格数据不足，需要 {} 根", symbol, self.slow_ma_period + 1);
                    continue;
                }

                // 计算当前和前一根的均线值，只在交叉时发出信号
                let fast_ma = Self::sma(&prices, self.fast_ma_period);
                let slow_ma = Self::sma(&prices, self.slow_ma_period);
                let prev_prices = &prices[..prices.len() - 1];
                let prev_fast = Self::sma(prev_prices, self.fast_ma_period);
                let prev_slow = Self::sma(prev_prices, self.slow_ma_period);

                if let (Some(fast), Some(slow), Some(pf), Some(ps)) =
                    (fast_ma, slow_ma, prev_fast, prev_slow)
                {
                    // 金叉：快线上穿慢线
                    if pf <= ps && fast > slow {
                        let quantity = 0.01;
                        signals.push(TradeSignal {
                            symbol: symbol.clone(),
                            market_id: ob.market_id,
                            side: Side::Buy,
                            price: mid_price,
                            quantity,
                            order_type: OrderType::Market,
                            reason: format!("金叉买入: 快均线 {:.2} 上穿慢均线 {:.2}", fast, slow),
                            timestamp: Utc::now(),
                        });
                    }
                    // 死叉：快线下穿慢线
                    else if pf >= ps && fast < slow {
                        let quantity = 0.01;
                        signals.push(TradeSignal {
                            symbol: symbol.clone(),
                            market_id: ob.market_id,
                            side: Side::Sell,
                            price: mid_price,
                            quantity,
                            order_type: OrderType::Market,
                            reason: format!("死叉卖出: 快均线 {:.2} 下穿慢均线 {:.2}", fast, slow),
                            timestamp: Utc::now(),
                        });
                    }

                    // 止损止盈检查
                    if let (Some(entry), Some(side)) = (self.position_entry, self.position_side) {
                        let pnl_pct = match side {
                            Side::Buy => (mid_price - entry) / entry,
                            Side::Sell => (entry - mid_price) / entry,
                        };
                        if pnl_pct <= -self.stop_loss_pct {
                            let exit_side = if side == Side::Buy { Side::Sell } else { Side::Buy };
                            signals.push(TradeSignal {
                                symbol: symbol.clone(),
                                market_id: ob.market_id,
                                side: exit_side,
                                price: mid_price,
                                quantity: 0.01,
                                order_type: OrderType::Market,
                                reason: format!("止损: PnL {:.2}%", pnl_pct * 100.0),
                                timestamp: Utc::now(),
                            });
                        } else if pnl_pct >= self.take_profit_pct {
                            let exit_side = if side == Side::Buy { Side::Sell } else { Side::Buy };
                            signals.push(TradeSignal {
                                symbol: symbol.clone(),
                                market_id: ob.market_id,
                                side: exit_side,
                                price: mid_price,
                                quantity: 0.01,
                                order_type: OrderType::Market,
                                reason: format!("止盈: PnL {:.2}%", pnl_pct * 100.0),
                                timestamp: Utc::now(),
                            });
                        }
                    }
                }
            }
        }

        if signals.is_empty() {
            Ok(None)
        } else {
            Ok(Some(signals))
        }
    }

    fn reset(&mut self) {
        self.price_history.clear();
        self.position_entry = None;
        self.position_side = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sma() {
        let prices = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(TrendStrategy::sma(&prices, 3), Some(4.0));
        assert_eq!(TrendStrategy::sma(&prices, 5), Some(3.0));
        assert_eq!(TrendStrategy::sma(&prices, 6), None);
    }

    #[test]
    fn test_trend_strategy_new() {
        let strategy = TrendStrategy::new(10, 30, 0.05, 0.1);
        assert_eq!(strategy.fast_ma_period, 10);
        assert_eq!(strategy.slow_ma_period, 30);
    }

    #[test]
    fn test_check_exit_stop_loss() {
        let mut strategy = TrendStrategy::new(10, 30, 0.05, 0.1);
        strategy.position_entry = Some(100.0);
        strategy.position_side = Some(Side::Buy);

        // 价格下跌超过5%应该触发止损
        assert!(strategy.check_exit(94.0).is_some());
        // 价格下跌不到5%不应该触发
        assert!(strategy.check_exit(96.0).is_none());
    }

    #[test]
    fn test_check_exit_take_profit() {
        let mut strategy = TrendStrategy::new(10, 30, 0.05, 0.1);
        strategy.position_entry = Some(100.0);
        strategy.position_side = Some(Side::Buy);

        // 价格上涨超过10%应该触发止盈
        assert!(strategy.check_exit(111.0).is_some());
        // 价格上涨不到10%不应该触发
        assert!(strategy.check_exit(108.0).is_none());
    }
}
