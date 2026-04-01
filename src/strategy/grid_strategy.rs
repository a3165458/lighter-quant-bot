use anyhow::Result;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{debug, info};

use crate::lighter::types::*;
use super::Strategy;

/// Grid Trading Strategy for Live Trading
///
/// Places limit buy orders below current price and limit sell orders above it.
/// When a grid level is filled, the opposite direction is unlocked for profit.
/// Each market maintains its own independent grid state.
/// Includes an EMA trend filter to avoid accumulating in strong trends.
pub struct GridStrategy {
    grid_count: usize,
    investment_per_grid: f64,
    price_deviation: f64,
    states: Mutex<HashMap<String, MarketGridState>>,
}

struct MarketGridState {
    anchor_price: f64,
    last_mid_price: f64,
    last_signal_time: Option<chrono::DateTime<Utc>>,
    filled_buy: Vec<bool>,
    filled_sell: Vec<bool>,
    /// Rolling price history for trend detection (up to 50 prices)
    price_history: Vec<f64>,
    /// EMA value
    ema: f64,
}

impl GridStrategy {
    pub fn new(grid_count: usize, investment_per_grid: f64, price_deviation: f64) -> Self {
        Self {
            grid_count,
            investment_per_grid,
            price_deviation,
            states: Mutex::new(HashMap::new()),
        }
    }

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
                Some(p) if p > 0.0 => p,
                _ => continue,
            };

            let half = self.grid_count / 2;
            let mut states = self.states.lock().unwrap();

            // Get or initialize per-market state
            let state = states.entry(symbol.clone()).or_insert_with(|| {
                info!("Grid anchor set: {:.2} for {}", mid_price, symbol);
                MarketGridState {
                    anchor_price: mid_price,
                    last_mid_price: mid_price,
                    last_signal_time: None,
                    filled_buy: vec![false; half],
                    filled_sell: vec![false; half],
                    price_history: vec![mid_price],
                    ema: mid_price,
                }
            });

            // Outlier filter: reject ticks >3% from last known price
            let tick_change = (mid_price - state.last_mid_price).abs() / state.last_mid_price;
            if tick_change > 0.03 {
                debug!("{} outlier tick rejected: {:.2} -> {:.2} ({:.2}%)",
                    symbol, state.last_mid_price, mid_price, tick_change * 100.0);
                continue;
            }
            state.last_mid_price = mid_price;

            // Update EMA (20-period exponential moving average)
            state.price_history.push(mid_price);
            if state.price_history.len() > 50 {
                state.price_history.remove(0);
            }
            let alpha = 2.0 / 21.0; // EMA-20
            state.ema = alpha * mid_price + (1.0 - alpha) * state.ema;

            // Trend strength: positive = price above EMA (bullish), negative = below (bearish)
            let trend_pct = (mid_price - state.ema) / state.ema;
            // Strong trend threshold: 0.3% deviation from EMA
            let strong_trend = trend_pct.abs() > 0.003;
            let bearish = trend_pct < -0.003;
            let bullish = trend_pct > 0.003;

            // Use market timestamps so backtests are throttled by simulated time, not wall clock.
            if let Some(last_signal_time) = state.last_signal_time {
                if ob.timestamp.signed_duration_since(last_signal_time) < Duration::seconds(15) {
                    continue;
                }
            }

            let anchor = state.anchor_price;

            // Only reset anchor if price drifted beyond 2x the full grid range
            let drift = (mid_price - anchor).abs() / anchor;
            if drift > self.price_deviation * 2.0 {
                state.anchor_price = mid_price;
                state.filled_buy = vec![false; half];
                state.filled_sell = vec![false; half];
                state.ema = mid_price; // reset EMA to avoid stale trend signal
                info!("Grid anchor reset: {:.2} -> {:.2} for {} (drift {:.2}%)",
                    anchor, mid_price, symbol, drift * 100.0);
                continue;
            }

            let (buy_grids, sell_grids) = self.grid_prices(anchor);

            // Check buy grids: price dropped to grid level
            // In strong bearish trend, only fill first buy level (reduce downside exposure)
            let mut signal_found = false;
            for (i, &grid_price) in buy_grids.iter().enumerate() {
                if i >= state.filled_buy.len() || state.filled_buy[i] {
                    continue;
                }
                // In strong downtrend, skip deeper buy levels to limit drawdown
                if bearish && i >= 2 {
                    continue;
                }
                if mid_price <= grid_price {
                    let quantity = self.investment_per_grid / grid_price;
                    all_signals.push(TradeSignal {
                        symbol: symbol.clone(),
                        market_id: ob.market_id,
                        side: Side::Buy,
                        price: grid_price,
                        quantity,
                        order_type: OrderType::Limit,
                        reason: format!("Grid Buy L{}: {:.2}", i + 1, grid_price),
                        timestamp: ob.timestamp,
                    });
                    state.filled_buy[i] = true;
                    if i < state.filled_sell.len() {
                        state.filled_sell[i] = false;
                    }
                    state.last_signal_time = Some(ob.timestamp);
                    signal_found = true;
                    break;
                }
            }

            // Check sell grids if no buy signal for this market
            // In strong bullish trend, skip deeper sell levels to let profits run
            if !signal_found {
                for (i, &grid_price) in sell_grids.iter().enumerate() {
                    if i >= state.filled_sell.len() || state.filled_sell[i] {
                        continue;
                    }
                    if bullish && i >= 2 {
                        continue;
                    }
                    if mid_price >= grid_price {
                        let quantity = self.investment_per_grid / grid_price;
                        all_signals.push(TradeSignal {
                            symbol: symbol.clone(),
                            market_id: ob.market_id,
                            side: Side::Sell,
                            price: grid_price,
                            quantity,
                            order_type: OrderType::Limit,
                            reason: format!("Grid Sell L{}: {:.2}", i + 1, grid_price),
                            timestamp: ob.timestamp,
                        });
                        state.filled_sell[i] = true;
                        if i < state.filled_buy.len() {
                            state.filled_buy[i] = false;
                        }
                        state.last_signal_time = Some(ob.timestamp);
                        break;
                    }
                }
            }

            debug!("{} mid={:.2} anchor={:.2} ema={:.2} trend={:+.3}% {}",
                symbol, mid_price, anchor, state.ema, trend_pct * 100.0,
                if strong_trend { if bearish { "↓BEAR" } else { "↑BULL" } } else { "→RANGE" });
        }

        if all_signals.is_empty() {
            Ok(None)
        } else {
            Ok(Some(all_signals))
        }
    }

    fn reset(&mut self) {
        let mut states = self.states.lock().unwrap();
        states.clear();
    }

    fn clear_filled_state(&self) {
        let mut states = self.states.lock().unwrap();
        for (symbol, state) in states.iter_mut() {
            let half = state.filled_buy.len();
            state.filled_buy = vec![false; half];
            state.filled_sell = vec![false; half];
            info!("Grid filled state cleared for {}", symbol);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn snapshot(symbol: &str, ts: i64, price: f64) -> MarketSnapshot {
        let mut snap = MarketSnapshot::default();
        snap.order_books.insert(
            symbol.to_string(),
            OrderBook {
                symbol: symbol.to_string(),
                market_id: 1,
                bids: vec![PriceLevel { price: price - 0.1, quantity: 1.0 }],
                asks: vec![PriceLevel { price: price + 0.1, quantity: 1.0 }],
                timestamp: Utc.timestamp_opt(ts, 0).unwrap(),
            },
        );
        snap
    }

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
        assert!(buy_grids[0] > buy_grids[1]);
        assert!(sell_grids[0] < sell_grids[1]);
    }

    #[tokio::test]
    async fn test_grid_cooldown_uses_market_time() {
        let strategy = GridStrategy::new(4, 100.0, 0.02);

        assert!(strategy.evaluate(&snapshot("BTC", 1_700_000_000, 100.0)).await.unwrap().is_none());

        let first = strategy.evaluate(&snapshot("BTC", 1_700_000_900, 98.5)).await.unwrap();
        assert_eq!(first.unwrap().len(), 1);

        let second = strategy.evaluate(&snapshot("BTC", 1_700_001_800, 97.5)).await.unwrap();
        assert_eq!(second.unwrap().len(), 1);
    }
}
