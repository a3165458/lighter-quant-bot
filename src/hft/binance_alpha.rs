//! Binance futures partial-depth OBI, used as the external alpha for crypto
//! names on Robinhood Lighter. Stocks have no Binance perp and stay on local OBI.
//!
//! Port of the imbalance + z-score in djienne `src/binance/obi.rs`, fed by the
//! public `@depth20@100ms` snapshot stream (no API key).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::time::{sleep, Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use crate::strategy::rolling::RollingStats;
use crate::strategy::vol_obi::{sum_ask_to, sum_bid_from};

const STALE_DEFAULT_MS: u64 = 5_000;
const LOOKING_DEPTH: f64 = 0.025;
const WINDOW: usize = 600;
const MIN_SAMPLES: u64 = 30;

/// Map an RH / Lighter base symbol onto a Binance USDT-M futures contract.
pub fn binance_futures_symbol(base: &str) -> Option<String> {
    let raw = base.trim();
    if raw.is_empty() {
        return None;
    }
    let upper = raw.to_ascii_uppercase();
    let ticker = upper
        .split(['/', '-', '_', ':'])
        .next()
        .unwrap_or(&upper)
        .trim();
    if ticker.is_empty() {
        return None;
    }
    // Equities / pre-IPO / ETFs have no Binance USDT-M book we can trust.
    if looks_like_equity(ticker) {
        return None;
    }
    Some(format!("{ticker}USDT"))
}

fn looks_like_equity(ticker: &str) -> bool {
    const EQUITY: &[&str] = &[
        "AAPL", "AMZN", "GOOGL", "GOOG", "META", "MSFT", "NVDA", "TSLA", "ORCL", "BABA", "BE",
        "CRWV", "COIN", "AMD", "INTC", "MU", "PLTR", "CRCL", "SNDK", "HOOD", "SPY", "QQQ", "SOXL",
        "SLV", "USO", "SGOV", "SPCX", "ANTHROPIC", "SKHY", "USAR",
    ];
    EQUITY.iter().any(|e| *e == ticker)
}

#[derive(Debug)]
pub struct SharedAlpha {
    bits: AtomicU64,
    updated_ms: AtomicU64,
    samples: AtomicU64,
}

impl SharedAlpha {
    fn new() -> Self {
        Self {
            bits: AtomicU64::new(0.0f64.to_bits()),
            updated_ms: AtomicU64::new(0),
            samples: AtomicU64::new(0),
        }
    }

    fn publish(&self, alpha: f64) {
        self.bits.store(alpha.to_bits(), Ordering::Relaxed);
        self.updated_ms.store(now_ms(), Ordering::Relaxed);
        self.samples.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get_if_fresh(&self, stale_ms: u64) -> Option<f64> {
        let updated = self.updated_ms.load(Ordering::Relaxed);
        if updated == 0 {
            return None;
        }
        if self.samples.load(Ordering::Relaxed) < MIN_SAMPLES {
            return None;
        }
        let age = now_ms().saturating_sub(updated);
        if age > stale_ms {
            return None;
        }
        Some(f64::from_bits(self.bits.load(Ordering::Relaxed)))
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Live hub: one combined Binance WS for every watched crypto symbol.
pub struct BinanceAlphaHub {
    alphas: Mutex<HashMap<String, Arc<SharedAlpha>>>,
    wanted: Mutex<Vec<String>>,
}

impl BinanceAlphaHub {
    pub fn new() -> Arc<Self> {
        let hub = Arc::new(Self {
            alphas: Mutex::new(HashMap::new()),
            wanted: Mutex::new(Vec::new()),
        });
        let worker = hub.clone();
        tokio::spawn(async move {
            worker.run_forever().await;
        });
        hub
    }

    pub fn watch(&self, symbols: &[String]) {
        let mut wanted = self.wanted.lock().unwrap();
        let mut changed = false;
        for s in symbols {
            if binance_futures_symbol(s).is_none() {
                continue;
            }
            if !wanted.iter().any(|w| w.eq_ignore_ascii_case(s)) {
                wanted.push(s.clone());
                changed = true;
            }
            self.alphas
                .lock()
                .unwrap()
                .entry(s.to_ascii_uppercase())
                .or_insert_with(|| Arc::new(SharedAlpha::new()));
        }
        if changed {
            debug!(count = wanted.len(), "binance alpha watch list updated");
        }
    }

    pub fn alpha(&self, symbol: &str, stale_secs: f64) -> Option<f64> {
        let key = symbol.to_ascii_uppercase();
        let alphas = self.alphas.lock().unwrap();
        let slot = alphas.get(&key)?;
        let stale_ms = (stale_secs.max(0.5) * 1000.0) as u64;
        slot.get_if_fresh(stale_ms.max(STALE_DEFAULT_MS / 2))
    }

    async fn run_forever(self: Arc<Self>) {
        loop {
            let streams = {
                let wanted = self.wanted.lock().unwrap().clone();
                wanted
                    .iter()
                    .filter_map(|s| binance_futures_symbol(s))
                    .map(|c| format!("{}@depth20@100ms", c.to_ascii_lowercase()))
                    .collect::<Vec<_>>()
            };
            if streams.is_empty() {
                sleep(Duration::from_secs(2)).await;
                continue;
            }
            let url = format!(
                "wss://fstream.binance.com/stream?streams={}",
                streams.join("/")
            );
            info!(streams = streams.len(), "connecting Binance OBI feed");
            match connect_async(&url).await {
                Ok((ws, _)) => {
                    let (mut write, mut read) = ws.split();
                    let mut stats: HashMap<String, RollingStats> = HashMap::new();
                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(Message::Text(text)) => {
                                self.on_message(&text, &mut stats);
                            }
                            Ok(Message::Ping(p)) => {
                                let _ = write.send(Message::Pong(p)).await;
                            }
                            Ok(Message::Close(_)) => break,
                            Err(e) => {
                                warn!("Binance OBI ws error: {e}");
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    warn!("Binance OBI connect failed: {e}");
                }
            }
            sleep(Duration::from_secs(2)).await;
        }
    }

    fn on_message(&self, text: &str, stats: &mut HashMap<String, RollingStats>) {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return;
        };
        let data = if v.get("data").is_some() { &v["data"] } else { &v };
        let contract = data
            .get("s")
            .and_then(|s| s.as_str())
            .or_else(|| {
                v.get("stream")
                    .and_then(|s| s.as_str())
                    .and_then(|s| s.split('@').next())
            })
            .unwrap_or("");
        let base = contract
            .to_ascii_uppercase()
            .trim_end_matches("USDT")
            .to_string();
        if base.is_empty() {
            return;
        }

        let bids = parse_levels(data, "bids").or_else(|| parse_levels(data, "b"));
        let asks = parse_levels(data, "asks").or_else(|| parse_levels(data, "a"));
        let (Some(bids), Some(asks)) = (bids, asks) else {
            return;
        };
        if bids.is_empty() || asks.is_empty() {
            return;
        }
        let best_bid = bids[0].0;
        let best_ask = asks[0].0;
        if best_bid <= 0.0 || best_ask <= best_bid {
            return;
        }
        let mid = (best_bid + best_ask) * 0.5;
        let imbalance =
            sum_bid_from(&bids, mid * (1.0 - LOOKING_DEPTH)) - sum_ask_to(&asks, mid * (1.0 + LOOKING_DEPTH));
        let slot = stats
            .entry(base.clone())
            .or_insert_with(|| RollingStats::new(WINDOW));
        slot.push(imbalance);
        let alpha = slot.zscore(imbalance);

        let shared = {
            let mut alphas = self.alphas.lock().unwrap();
            alphas
                .entry(base)
                .or_insert_with(|| Arc::new(SharedAlpha::new()))
                .clone()
        };
        shared.publish(alpha);
    }
}

fn parse_levels(data: &Value, key: &str) -> Option<Vec<(f64, f64)>> {
    let arr = data.get(key)?.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for row in arr {
        let pair = row.as_array()?;
        if pair.len() < 2 {
            continue;
        }
        let px = pair[0].as_str().and_then(|s| s.parse().ok())?;
        let qty = pair[1].as_str().and_then(|s| s.parse().ok())?;
        if px > 0.0 && qty >= 0.0 {
            out.push((px, qty));
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_crypto_not_stocks() {
        assert_eq!(binance_futures_symbol("BTC").as_deref(), Some("BTCUSDT"));
        assert_eq!(binance_futures_symbol("ETH/USDG").as_deref(), Some("ETHUSDT"));
        assert_eq!(binance_futures_symbol("HYPE").as_deref(), Some("HYPEUSDT"));
        assert_eq!(binance_futures_symbol("NEAR").as_deref(), Some("NEARUSDT"));
        assert_eq!(binance_futures_symbol("VVV").as_deref(), Some("VVVUSDT"));
        assert!(binance_futures_symbol("AAPL").is_none());
        assert!(binance_futures_symbol("BABA").is_none());
        assert!(binance_futures_symbol("CRWV").is_none());
        assert!(binance_futures_symbol("BE").is_none());
        assert!(binance_futures_symbol("NVDA").is_none());
        assert!(binance_futures_symbol("SPY").is_none());
    }

    #[test]
    fn stale_alpha_is_none_until_published() {
        let a = SharedAlpha::new();
        assert!(a.get_if_fresh(5_000).is_none());
        for _ in 0..MIN_SAMPLES {
            a.publish(1.25);
        }
        assert!((a.get_if_fresh(5_000).unwrap() - 1.25).abs() < 1e-12);
    }
}
