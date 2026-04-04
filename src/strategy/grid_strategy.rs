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
///
/// Features:
/// - Multi-tier EMA trend filter (blocks all buys in very strong downtrend)
/// - Max accumulated position limit (caps filled levels per side)
/// - Trailing anchor that gradually drifts toward EMA
/// - Faster anchor reset at 1.5x grid range
pub struct GridStrategy {
    grid_count: usize,
    investment_per_grid: f64,
    price_deviation: f64,
    /// Max filled grid levels per side before blocking new signals
    max_filled_per_side: usize,
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
        // Cap max filled per side: at most half of total grid levels, minimum 3
        let half = grid_count / 2;
        let max_filled = half.min(5).max(3);
        Self {
            grid_count,
            investment_per_grid,
            price_deviation,
            max_filled_per_side: max_filled,
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

            // Multi-tier trend detection
            let trend_pct = (mid_price - state.ema) / state.ema;
            // Tier 1: Very strong trend (>0.6% from EMA) — block ALL counter-trend signals
            let very_bearish = trend_pct < -0.006;
            let very_bullish = trend_pct > 0.006;
            // Tier 2: Strong trend (>0.3% from EMA) — only allow nearest level
            let bearish = trend_pct < -0.003;
            let bullish = trend_pct > 0.003;

            // Use market timestamps so backtests are throttled by simulated time, not wall clock.
            if let Some(last_signal_time) = state.last_signal_time {
                if ob.timestamp.signed_duration_since(last_signal_time) < Duration::seconds(15) {
                    continue;
                }
            }

            // Trailing anchor: gradually blend toward EMA to keep grid centered on market
            let anchor_drift_rate = 0.0005; // 0.05% per tick toward EMA
            state.anchor_price = state.anchor_price * (1.0 - anchor_drift_rate)
                + state.ema * anchor_drift_rate;
            let anchor = state.anchor_price;

            // Reset anchor if price drifted beyond 1.5x the full grid range (faster than 2x)
            let drift = (mid_price - anchor).abs() / anchor;
            if drift > self.price_deviation * 1.5 {
                state.anchor_price = mid_price;
                state.filled_buy = vec![false; half];
                state.filled_sell = vec![false; half];
                state.ema = mid_price;
                info!("Grid anchor reset: {:.2} -> {:.2} for {} (drift {:.2}%)",
                    anchor, mid_price, symbol, drift * 100.0);
                continue;
            }

            let (buy_grids, sell_grids) = self.grid_prices(anchor);

            // Count currently filled levels per side
            let filled_buy_count = state.filled_buy.iter().filter(|&&f| f).count();
            let filled_sell_count = state.filled_sell.iter().filter(|&&f| f).count();

            // Only trust aggressive trend tiers after enough EMA history (avoids false signals on init)
            let has_enough_history = state.price_history.len() >= 10;

            // Check buy grids: price dropped to grid level
            let mut signal_found = false;
            for (i, &grid_price) in buy_grids.iter().enumerate() {
                if i >= state.filled_buy.len() || state.filled_buy[i] {
                    continue;
                }
                // Multi-tier trend filter for buys:
                // Very strong downtrend → block ALL buys (only after enough EMA data)
                if has_enough_history && very_bearish {
                    continue;
                }
                // Strong downtrend → only allow nearest buy level (L0)
                if bearish && i >= 1 {
                    continue;
                }
                // Max accumulated position limit per side
                if filled_buy_count >= self.max_filled_per_side {
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
            if !signal_found {
                for (i, &grid_price) in sell_grids.iter().enumerate() {
                    if i >= state.filled_sell.len() || state.filled_sell[i] {
                        continue;
                    }
                    // Multi-tier trend filter for sells:
                    // Very strong uptrend → block ALL sells (only after enough EMA data)
                    if has_enough_history && very_bullish {
                        continue;
                    }
                    // Strong uptrend → only allow nearest sell level (L0)
                    if bullish && i >= 1 {
                        continue;
                    }
                    // Max accumulated position limit per side
                    if filled_sell_count >= self.max_filled_per_side {
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

            debug!("{} mid={:.2} anchor={:.2} ema={:.2} trend={:+.3}% {} filled_buy={} filled_sell={}",
                symbol, mid_price, anchor, state.ema, trend_pct * 100.0,
                if very_bearish { "⬇VERY_BEAR" } else if bearish { "↓BEAR" }
                else if very_bullish { "⬆VERY_BULL" } else if bullish { "↑BULL" }
                else { "→RANGE" },
                filled_buy_count, filled_sell_count);
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
        // half=2, buy grids: [99.0, 98.0], sell grids: [101.0, 102.0]

        // Initial eval: sets anchor at 100.0 (no grid level hit)
        assert!(strategy.evaluate(&snapshot("BTC", 1_700_000_000, 100.0)).await.unwrap().is_none());

        // Price drops to 98.5 → hits buy L0 at 99.0
        let first = strategy.evaluate(&snapshot("BTC", 1_700_000_900, 98.5)).await.unwrap();
        assert!(first.is_some(), "Should trigger buy signal");
        assert_eq!(first.unwrap().len(), 1);

        // Only 5 seconds later → blocked by 15s cooldown
        let blocked = strategy.evaluate(&snapshot("BTC", 1_700_000_905, 98.5)).await.unwrap();
        assert!(blocked.is_none(), "Should be blocked by cooldown");

        // After cooldown (900s later) at same price → L0 already filled, L1 blocked by trend filter
        let after_cooldown = strategy.evaluate(&snapshot("BTC", 1_700_001_800, 98.5)).await.unwrap();
        assert!(after_cooldown.is_none(), "L1 should be blocked by bearish trend filter");
    }

    #[tokio::test]
    async fn test_grid_trend_filter_blocks_deep_buys() {
        let strategy = GridStrategy::new(6, 100.0, 0.03);
        // half=3, step = 100 * 0.03 / 3 = 1.0
        // buy grids: ~[99.0, 98.0, 97.0]

        // Set anchor
        strategy.evaluate(&snapshot("BTC", 1_700_000_000, 100.0)).await.unwrap();

        // Price drops to clearly hit buy L0 (bearish but L0 allowed)
        let sig = strategy.evaluate(&snapshot("BTC", 1_700_000_100, 98.5)).await.unwrap();
        assert!(sig.is_some(), "Buy L0 should be allowed even in bearish trend");

        // Price drops further → L1 blocked by bearish filter (i >= 1)
        let sig2 = strategy.evaluate(&snapshot("BTC", 1_700_000_200, 97.5)).await.unwrap();
        assert!(sig2.is_none(), "Buy L1 should be blocked by bearish trend filter");
    }
}
