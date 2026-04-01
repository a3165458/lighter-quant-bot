use anyhow::Result;
use async_trait::async_trait;
use chrono::{Duration, Utc};
use std::sync::Mutex;
use tracing::{debug, info};

use crate::lighter::types::*;
use super::Strategy;

/// DCA (Dollar-Cost Averaging) Strategy
///
/// Places periodic buy orders at fixed intervals. Optionally accelerates
/// buying when price drops below a configurable "dip threshold" relative
/// to the rolling average price.
pub struct DcaStrategy {
    /// Interval between buys (in seconds)
    buy_interval_secs: i64,
    /// Fixed USD amount per buy
    amount_per_buy: f64,
    /// Dip threshold as a fraction (e.g. 0.02 = 2%). When price is this
    /// much below the rolling average, an extra buy is triggered.
    dip_threshold: f64,
    state: Mutex<DcaState>,
}

struct DcaState {
    last_buy_time: Option<chrono::DateTime<Utc>>,
    /// Rolling average of observed prices (simple accumulation)
    avg_price: f64,
    price_count: u64,
    /// Whether a dip-buy was already triggered at the current dip level
    dip_bought: bool,
}

impl DcaStrategy {
    pub fn new(buy_interval_hours: f64, amount_per_buy: f64, dip_threshold_pct: f64) -> Self {
        Self {
            buy_interval_secs: (buy_interval_hours * 3600.0) as i64,
            amount_per_buy,
            dip_threshold: dip_threshold_pct / 100.0,
            state: Mutex::new(DcaState {
                last_buy_time: None,
                avg_price: 0.0,
                price_count: 0,
                dip_bought: false,
            }),
        }
    }
}

#[async_trait]
impl Strategy for DcaStrategy {
    fn name(&self) -> &str {
        "dca"
    }

    async fn evaluate(&self, snapshot: &MarketSnapshot) -> Result<Option<Vec<TradeSignal>>> {
        let mut signals = Vec::new();
        let mut state = self.state.lock().unwrap();

        for (symbol, ob) in &snapshot.order_books {
            let mid_price = match ob.mid_price() {
                Some(p) if p > 0.0 => p,
                _ => continue,
            };

            // Update rolling average
            state.price_count += 1;
            state.avg_price += (mid_price - state.avg_price) / state.price_count as f64;

            let now = ob.timestamp;
            let quantity = self.amount_per_buy / mid_price;

            // Check interval-based buy
            let interval_buy = match state.last_buy_time {
                None => true, // First ever → buy
                Some(last) => now.signed_duration_since(last) >= Duration::seconds(self.buy_interval_secs),
            };

            // Check dip buy: price dropped below avg by dip_threshold
            let dip_pct = if state.avg_price > 0.0 {
                (state.avg_price - mid_price) / state.avg_price
            } else {
                0.0
            };
            let dip_buy = self.dip_threshold > 0.0 && dip_pct >= self.dip_threshold && !state.dip_bought;

            if interval_buy || dip_buy {
                let reason = if dip_buy {
                    format!("DCA Dip Buy: price {:.2} is {:.1}% below avg {:.2}",
                        mid_price, dip_pct * 100.0, state.avg_price)
                } else {
                    format!("DCA Interval Buy: {:.2} (every {}h)",
                        mid_price, self.buy_interval_secs / 3600)
                };

                info!("{} on {}", reason, symbol);

                signals.push(TradeSignal {
                    symbol: symbol.clone(),
                    market_id: ob.market_id,
                    side: Side::Buy,
                    price: mid_price,
                    quantity,
                    order_type: OrderType::Limit,
                    reason,
                    timestamp: now,
                });

                state.last_buy_time = Some(now);
                if dip_buy {
                    state.dip_bought = true;
                }
            }

            // Reset dip flag when price recovers above avg
            if dip_pct < self.dip_threshold * 0.5 {
                state.dip_bought = false;
            }

            debug!("{} DCA: price={:.2}, avg={:.2}, dip={:.2}%, next_buy_in={}s",
                symbol, mid_price, state.avg_price, dip_pct * 100.0,
                state.last_buy_time.map(|t| self.buy_interval_secs - now.signed_duration_since(t).num_seconds()).unwrap_or(0));
        }

        if signals.is_empty() {
            Ok(None)
        } else {
            Ok(Some(signals))
        }
    }

    fn reset(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.last_buy_time = None;
        state.avg_price = 0.0;
        state.price_count = 0;
        state.dip_bought = false;
    }

    fn clear_filled_state(&self) {
        let mut state = self.state.lock().unwrap();
        state.last_buy_time = None;
        state.dip_bought = false;
        info!("DCA state cleared");
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
                bids: vec![PriceLevel { price: price - 0.5, quantity: 1.0 }],
                asks: vec![PriceLevel { price: price + 0.5, quantity: 1.0 }],
                timestamp: Utc.timestamp_opt(ts, 0).unwrap(),
            },
        );
        snap
    }

    #[tokio::test]
    async fn test_dca_first_buy() {
        let strategy = DcaStrategy::new(4.0, 10.0, 2.0);
        let result = strategy.evaluate(&snapshot("BTC", 1_700_000_000, 50000.0)).await.unwrap();
        assert!(result.is_some());
        let signals = result.unwrap();
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].side, Side::Buy);
    }

    #[tokio::test]
    async fn test_dca_interval_throttle() {
        let strategy = DcaStrategy::new(1.0, 10.0, 0.0); // 1h interval, no dip
        // First buy
        let r1 = strategy.evaluate(&snapshot("BTC", 1_700_000_000, 50000.0)).await.unwrap();
        assert!(r1.is_some());
        // 30 min later - should NOT buy
        let r2 = strategy.evaluate(&snapshot("BTC", 1_700_001_800, 50000.0)).await.unwrap();
        assert!(r2.is_none());
        // 61 min later - should buy
        let r3 = strategy.evaluate(&snapshot("BTC", 1_700_003_660, 50000.0)).await.unwrap();
        assert!(r3.is_some());
    }

    #[tokio::test]
    async fn test_dca_dip_buy() {
        let strategy = DcaStrategy::new(24.0, 10.0, 3.0); // 24h interval, 3% dip
        // First buy at 50000
        strategy.evaluate(&snapshot("BTC", 1_700_000_000, 50000.0)).await.unwrap();
        // Feed many prices around 50000 to build avg
        for i in 1..20 {
            strategy.evaluate(&snapshot("BTC", 1_700_000_000 + i * 60, 50000.0)).await.unwrap();
        }
        // Price drops 4% (48000) → should trigger dip buy even though interval hasn't elapsed
        let r = strategy.evaluate(&snapshot("BTC", 1_700_001_200, 48000.0)).await.unwrap();
        assert!(r.is_some());
    }
}
