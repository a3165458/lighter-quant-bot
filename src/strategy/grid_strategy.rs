use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Mutex;
use tracing::debug;

use crate::lighter::types::*;
use super::Strategy;

/// 网格交易策略
///
/// 在指定价格范围内均匀分布买卖网格，当价格触及网格线时进行交易。
/// 使用锚定价格保持网格稳定，仅在价格偏离过大时重新锚定。
pub struct GridStrategy {
    grid_count: usize,
    investment_per_grid: f64,
    price_deviation: f64,
    /// 使用 Mutex 实现内部可变性，因为 Strategy::evaluate 接受 &self
    state: Mutex<GridState>,
}

struct GridState {
    anchor_price: Option<f64>,
    filled_buy: Vec<bool>,
    filled_sell: Vec<bool>,
}

impl GridStrategy {
    pub fn new(grid_count: usize, investment_per_grid: f64, price_deviation: f64) -> Self {
        let half = grid_count / 2;
        Self {
            grid_count,
            investment_per_grid,
            price_deviation,
            state: Mutex::new(GridState {
                anchor_price: None,
                filled_buy: vec![false; half],
                filled_sell: vec![false; half],
            }),
        }
    }

    /// 计算网格线价格
    fn grid_prices(&self, anchor: f64) -> (Vec<f64>, Vec<f64>) {
        let half = self.grid_count / 2;
        let step = anchor * self.price_deviation / half.max(1) as f64;
        let buy_grids: Vec<f64> = (1..=half).map(|i| anchor - i as f64 * step).collect();
        let sell_grids: Vec<f64> = (1..=half).map(|i| anchor + i as f64 * step).collect();
        (buy_grids, sell_grids)
    }
}

#[async_trait]
impl Strategy for GridStrategy {
    fn name(&self) -> &str {
        "grid_trading"
    }

    async fn evaluate(&self, snapshot: &MarketSnapshot) -> Result<Option<Vec<TradeSignal>>> {
        let mut all_signals = Vec::new();

        for (symbol, ob) in &snapshot.order_books {
            let mid_price = match ob.mid_price() {
                Some(p) => p,
                None => continue,
            };

            // 需要至少2根K线来检测价格穿越
            let candles = match snapshot.candles.get(symbol) {
                Some(c) if c.len() >= 2 => c,
                _ => continue,
            };

            let prev = &candles[candles.len() - 2];
            let cur = &candles[candles.len() - 1];

            let mut state = self.state.lock().unwrap();

            // 初始化或重置锚定价格（价格偏离过大时重置网格）
            let need_reset = match state.anchor_price {
                None => true,
                Some(anchor) => {
                    let drift = (mid_price - anchor).abs() / anchor;
                    drift > self.price_deviation * 0.8
                }
            };
            if need_reset {
                state.anchor_price = Some(mid_price);
                let half = self.grid_count / 2;
                state.filled_buy = vec![false; half];
                state.filled_sell = vec![false; half];
                debug!("网格锚定重置: {:.2}", mid_price);
                continue;
            }

            let anchor = state.anchor_price.unwrap();
            let (buy_grids, sell_grids) = self.grid_prices(anchor);

            // 检查买入网格：当前K线低点穿过网格线，且前一根收盘在网格线上方
            for (i, &grid_price) in buy_grids.iter().enumerate() {
                if i >= state.filled_buy.len() || state.filled_buy[i] {
                    continue;
                }
                if cur.low <= grid_price && prev.close > grid_price {
                    let quantity = self.investment_per_grid / grid_price;
                    all_signals.push(TradeSignal {
                        symbol: symbol.clone(),
                        side: Side::Buy,
                        price: grid_price,
                        quantity,
                        order_type: OrderType::Limit,
                        reason: format!("网格买入L{}: {:.2}", i + 1, grid_price),
                        timestamp: Utc::now(),
                    });
                    state.filled_buy[i] = true;
                    // 解锁对面的卖出网格，实现网格来回交易获利
                    if i < state.filled_sell.len() {
                        state.filled_sell[i] = false;
                    }
                    break; // 每个tick最多触发一个信号
                }
            }

            // 如果没有买入信号，检查卖出网格
            if all_signals.is_empty() {
                for (i, &grid_price) in sell_grids.iter().enumerate() {
                    if i >= state.filled_sell.len() || state.filled_sell[i] {
                        continue;
                    }
                    if cur.high >= grid_price && prev.close < grid_price {
                        let quantity = self.investment_per_grid / grid_price;
                        all_signals.push(TradeSignal {
                            symbol: symbol.clone(),
                            side: Side::Sell,
                            price: grid_price,
                            quantity,
                            order_type: OrderType::Limit,
                            reason: format!("网格卖出L{}: {:.2}", i + 1, grid_price),
                            timestamp: Utc::now(),
                        });
                        state.filled_sell[i] = true;
                        if i < state.filled_buy.len() {
                            state.filled_buy[i] = false;
                        }
                        break;
                    }
                }
            }
        }

        if all_signals.is_empty() {
            Ok(None)
        } else {
            Ok(Some(all_signals))
        }
    }

    fn reset(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.anchor_price = None;
        state.filled_buy.fill(false);
        state.filled_sell.fill(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grid_prices() {
        let strategy = GridStrategy::new(10, 100.0, 0.02);
        let (buy_grids, sell_grids) = strategy.grid_prices(50000.0);
        assert_eq!(buy_grids.len(), 5);
        assert_eq!(sell_grids.len(), 5);
        assert!(buy_grids.iter().all(|p| *p < 50000.0));
        assert!(sell_grids.iter().all(|p| *p > 50000.0));
    }

    #[test]
    fn test_grid_symmetry() {
        let strategy = GridStrategy::new(20, 50.0, 0.015);
        let (buy_grids, sell_grids) = strategy.grid_prices(10000.0);
        assert_eq!(buy_grids.len(), 10);
        assert_eq!(sell_grids.len(), 10);
        // First buy grid is closest to anchor
        assert!(buy_grids[0] > buy_grids[1]);
        // First sell grid is closest to anchor
        assert!(sell_grids[0] < sell_grids[1]);
    }
}
