use anyhow::{bail, Result};
use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::debug;

use super::grid_strategy::InventoryMode;
use super::inventory_bias::{
    apply_inventory_exit_bias, apply_quality_spread_multiplier, InventoryExitBias,
};
use super::vol_obi::{
    fallback_reduce_only, tick_size_from_levels, VolObiCalculator, VolObiConfig,
};
use super::Strategy;
use crate::hft::BinanceAlphaHub;
use crate::lighter::types::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteEngine {
    /// Original Hummingbot simple_pmm (join-BBO + configured offset).
    Simple,
    /// djienne vol + OBI (external Binance alpha on crypto).
    VolObi,
}

impl QuoteEngine {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "simple" | "pmm" | "join" => Ok(Self::Simple),
            "vol_obi" | "vol-obi" | "djienne" | "obi" => Ok(Self::VolObi),
            other => bail!("unknown quote_engine '{other}' (simple|vol_obi)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaSource {
    Local,
    Binance,
}

impl AlphaSource {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "" | "local" | "book" | "none" => Ok(Self::Local),
            "binance" => Ok(Self::Binance),
            other => bail!("unknown alpha_source '{other}' (local|binance)"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VolObiMmSettings {
    pub vol_obi: VolObiConfig,
    pub alpha_source: AlphaSource,
    pub alpha_stale_secs: f64,
    pub inventory_exit: InventoryExitBias,
    pub quality_multiplier: f64,
}

impl Default for VolObiMmSettings {
    fn default() -> Self {
        Self {
            vol_obi: VolObiConfig::default(),
            alpha_source: AlphaSource::Binance,
            alpha_stale_secs: 5.0,
            inventory_exit: InventoryExitBias::default(),
            quality_multiplier: 1.0,
        }
    }
}

const DUST_NOTIONAL: f64 = 1.0;

/// Hummingbot-style simple PMM parameters (fractional spreads, not bps).
#[derive(Debug, Clone, Copy)]
pub struct MmQuoteParams {
    pub bid_spread: f64,
    pub ask_spread: f64,
    pub order_notional: f64,
    /// How far inventory can push the quotes. 0 = no skew.
    pub inventory_skew: f64,
    /// Absolute inventory (base qty) that maps to full skew. Must be > 0 to skew.
    pub inventory_target: f64,
    /// Hard inventory cap in base qty. Same-side adding quotes are dropped at/above this.
    pub max_inventory: f64,
    /// Minimum time between cancel-replace cycles on one symbol.
    pub min_requote_secs: i64,
}

impl Default for MmQuoteParams {
    fn default() -> Self {
        Self {
            bid_spread: 0.001,
            ask_spread: 0.001,
            order_notional: 50.0,
            inventory_skew: 0.5,
            inventory_target: 0.01,
            max_inventory: 0.05,
            min_requote_secs: 10,
        }
    }
}

/// Two-sided maker quote around a mid, after inventory skew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwoSidedQuote {
    pub mid: f64,
    pub bid_price: f64,
    pub ask_price: f64,
    pub bid_qty: f64,
    pub ask_qty: f64,
    pub expected_edge_bps: f64,
}

/// Mid from a two-sided book. Rejects non-positive or crossed books.
pub fn mid_from_bbo(bid: f64, ask: f64) -> Option<f64> {
    if !bid.is_finite() || !ask.is_finite() {
        return None;
    }
    if bid <= 0.0 || ask <= 0.0 {
        return None;
    }
    if ask < bid {
        return None;
    }
    let mid = (bid + ask) / 2.0;
    (mid > 0.0).then_some(mid)
}

/// Inventory-aware two-sided maker quote.
///
/// Mechanics follow Hummingbot `simple_pmm`: quotes sit a configured fraction
/// off mid, then inventory skew makes the reducing side more aggressive and
/// the adding side less aggressive (price and size).
///
/// Long inventory (positive) → lower ask (more aggressive sell) and lower bid
/// (less aggressive buy). Short inventory is the reverse.
pub fn quote_two_sided(
    bid: f64,
    ask: f64,
    inventory: f64,
    params: &MmQuoteParams,
) -> Option<TwoSidedQuote> {
    if params.bid_spread < 0.0 || params.ask_spread < 0.0 {
        return None;
    }
    if params.order_notional <= 0.0 {
        return None;
    }
    let mid = mid_from_bbo(bid, ask)?;

    let inv_ratio = if params.inventory_target > 0.0 && params.inventory_skew != 0.0 {
        (inventory / params.inventory_target).clamp(-1.0, 1.0) * params.inventory_skew
    } else {
        0.0
    };

    // Floor offset from mid (keeps the profitability gate happy on tight books).
    // If the live BBO is already inside that floor, join it — that is how
    // maker volume happens. Standing 10 bps behind BTC's 1–2 bps book never fills.
    let bid_spread = (params.bid_spread * (1.0 + inv_ratio)).max(0.0);
    let ask_spread = (params.ask_spread * (1.0 - inv_ratio)).max(0.0);
    let floor_bid = mid * (1.0 - bid_spread);
    let floor_ask = mid * (1.0 + ask_spread);
    let mut bid_price = bid.max(floor_bid);
    let mut ask_price = ask.min(floor_ask);
    // Inventory: long → push bid away / pull ask in (and the reverse when short).
    let shift = inv_ratio * (ask - bid).abs() * 0.25;
    bid_price -= shift;
    ask_price -= shift;
    if bid_price <= 0.0 || ask_price <= bid_price {
        return None;
    }

    // Size skew: long → smaller bid, larger ask (and the reverse when short).
    let base_qty = params.order_notional / mid;
    let bid_qty = (base_qty * (1.0 - 0.5 * inv_ratio)).max(0.0);
    let ask_qty = (base_qty * (1.0 + 0.5 * inv_ratio)).max(0.0);

    let expected_edge_bps = ((ask_price - bid_price) / mid) * 10_000.0;
    Some(TwoSidedQuote {
        mid,
        bid_price,
        ask_price,
        bid_qty,
        ask_qty,
        expected_edge_bps,
    })
}

/// Apply a hard inventory cap: drop the same-side adding quote at/above the cap.
pub fn apply_inventory_cap(
    quote: TwoSidedQuote,
    inventory: f64,
    max_inventory: f64,
    mode: InventoryMode,
) -> Option<TwoSidedQuote> {
    if mode == InventoryMode::ResearchNoCap || max_inventory <= 0.0 {
        return Some(quote);
    }
    let mut q = quote;
    if inventory >= max_inventory {
        q.bid_qty = 0.0;
    }
    if inventory <= -max_inventory {
        q.ask_qty = 0.0;
    }
    if q.bid_qty <= 0.0 && q.ask_qty <= 0.0 {
        return None;
    }
    Some(q)
}

/// Whether to emit a new two-sided quote. Never more often than `min_interval`,
/// and only if mid moved a meaningful fraction of the spread, inventory changed,
/// or the last quote is older than 30s (refresh).
pub fn should_requote(
    prev_mid: f64,
    prev_inv: f64,
    prev_ts: DateTime<Utc>,
    mid: f64,
    inv: f64,
    now: DateTime<Utc>,
    bid_spread: f64,
    min_interval_secs: i64,
) -> bool {
    let elapsed = now.signed_duration_since(prev_ts);
    let min_interval = ChronoDuration::seconds(min_interval_secs.max(1));
    if elapsed < min_interval {
        return false;
    }
    let mid_moved =
        prev_mid > 0.0 && mid > 0.0 && (mid - prev_mid).abs() / prev_mid >= bid_spread.max(1e-6) * 0.25;
    let inv_changed = (inv - prev_inv).abs() > 1e-9;
    mid_moved || inv_changed || elapsed >= ChronoDuration::seconds(30)
}

/// Quote from an order book + signed inventory. Shared by evaluate and tests.
pub fn quote_from_book(
    book: &OrderBook,
    inventory: f64,
    params: &MmQuoteParams,
    mode: InventoryMode,
) -> Option<TwoSidedQuote> {
    let bid = book.best_bid()?;
    let ask = book.best_ask()?;
    let quote = quote_two_sided(bid, ask, inventory, params)?;
    apply_inventory_cap(quote, inventory, params.max_inventory, mode)
}

pub struct MarketMakingStrategy {
    params: MmQuoteParams,
    inventory_mode: InventoryMode,
    engine: QuoteEngine,
    vol_settings: VolObiMmSettings,
    /// Last (mid, inventory, quote time) per symbol.
    last_quotes: Mutex<HashMap<String, (f64, f64, DateTime<Utc>)>>,
    vol_engines: Mutex<HashMap<String, VolObiCalculator>>,
    binance: Option<Arc<BinanceAlphaHub>>,
}

impl MarketMakingStrategy {
    pub fn new(params: MmQuoteParams, inventory_mode: InventoryMode) -> Result<Self> {
        Self::with_engine(
            params,
            inventory_mode,
            QuoteEngine::Simple,
            VolObiMmSettings::default(),
        )
    }

    pub fn with_engine(
        params: MmQuoteParams,
        inventory_mode: InventoryMode,
        engine: QuoteEngine,
        vol_settings: VolObiMmSettings,
    ) -> Result<Self> {
        if params.bid_spread < 0.0 || params.ask_spread < 0.0 {
            bail!("bid_spread/ask_spread must be >= 0");
        }
        if params.order_notional <= 0.0 {
            bail!("order_notional must be > 0");
        }
        if params.inventory_skew < 0.0 {
            bail!("inventory_skew must be >= 0");
        }
        let binance = if engine == QuoteEngine::VolObi
            && vol_settings.alpha_source == AlphaSource::Binance
            && tokio::runtime::Handle::try_current().is_ok()
        {
            Some(BinanceAlphaHub::new())
        } else {
            None
        };
        Ok(Self {
            params,
            inventory_mode,
            engine,
            vol_settings,
            last_quotes: Mutex::new(HashMap::new()),
            vol_engines: Mutex::new(HashMap::new()),
            binance,
        })
    }

    #[allow(dead_code)]
    pub fn quote_engine(&self) -> QuoteEngine {
        self.engine
    }

    fn levels(book: &OrderBook) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
        let bids = book
            .bids
            .iter()
            .map(|l| (l.price, l.quantity))
            .collect::<Vec<_>>();
        let asks = book
            .asks
            .iter()
            .map(|l| (l.price, l.quantity))
            .collect::<Vec<_>>();
        (bids, asks)
    }

    fn quote_vol_obi(&self, book: &OrderBook, inventory: f64) -> Option<TwoSidedQuote> {
        let bid = book.best_bid()?;
        let ask = book.best_ask()?;
        let mid = mid_from_bbo(bid, ask)?;
        // Incremental/partial books can print a deep leftover level as BBO
        // (dashboard already skips >50 bps). Feeding those into vol_obi
        // produces multi-percent quotes.
        if (ask - bid) / mid > 0.02 {
            return None;
        }
        let (bids, asks) = Self::levels(book);
        let tick = tick_size_from_levels(&bids, &asks, mid);
        let max_pos_usd = if self.params.max_inventory > 0.0 {
            self.params.max_inventory * mid
        } else {
            0.0
        };
        if let Some(hub) = &self.binance {
            hub.watch(std::slice::from_ref(&book.symbol));
        }
        let now_ns = book.timestamp.timestamp() * 1_000_000_000
            + i64::from(book.timestamp.timestamp_subsec_nanos());

        let mut engines = self.vol_engines.lock().unwrap();
        let calc = engines.entry(book.symbol.clone()).or_insert_with(|| {
            VolObiCalculator::new(&self.vol_settings.vol_obi, tick, max_pos_usd)
        });
        calc.set_max_position_dollar(max_pos_usd);
        let ext = self
            .binance
            .as_ref()
            .and_then(|hub| hub.alpha(&book.symbol, self.vol_settings.alpha_stale_secs));
        calc.set_alpha_override(ext);
        calc.on_book_update(now_ns, mid, &bids, &asks);

        let (mut bid_price, mut ask_price) = match calc.quote(mid, inventory) {
            Some(pair) => pair,
            None => {
                let (fb, fa) = fallback_reduce_only(
                    mid,
                    inventory,
                    tick,
                    self.vol_settings.vol_obi.min_half_spread_bps,
                )?;
                (fb, fa)
            }
        };

        let (qb, qa) = apply_quality_spread_multiplier(
            bid_price,
            ask_price,
            mid,
            self.vol_settings.quality_multiplier,
            tick,
        );
        bid_price = qb;
        ask_price = qa;
        let (qb, qa) = apply_inventory_exit_bias(
            bid_price,
            ask_price,
            mid,
            inventory,
            max_pos_usd,
            0.0,
            2.0,
            &self.vol_settings.inventory_exit,
            tick,
        );
        bid_price = qb;
        ask_price = qa;

        let base_qty = self.params.order_notional / mid;
        let mut bid_qty = if bid_price.is_finite() && bid_price > 0.0 {
            base_qty
        } else {
            0.0
        };
        let mut ask_qty = if ask_price.is_finite() && ask_price > 0.0 {
            base_qty
        } else {
            0.0
        };
        if inventory > 0.0 {
            ask_qty *= 1.15;
            bid_qty *= 0.85;
        } else if inventory < 0.0 {
            bid_qty *= 1.15;
            ask_qty *= 0.85;
        }

        if !bid_price.is_finite() || bid_price <= 0.0 {
            bid_qty = 0.0;
        }
        if !ask_price.is_finite() || ask_price <= 0.0 {
            ask_qty = 0.0;
        }
        if bid_qty > 0.0 && ask_qty > 0.0 && bid_price >= ask_price {
            return None;
        }
        let width = if bid_qty > 0.0 && ask_qty > 0.0 {
            ((ask_price - bid_price) / mid) * 10_000.0
        } else {
            self.vol_settings.vol_obi.min_half_spread_bps * 2.0
        };
        Some(TwoSidedQuote {
            mid,
            bid_price: if bid_qty > 0.0 { bid_price } else { 0.0 },
            ask_price: if ask_qty > 0.0 { ask_price } else { 0.0 },
            bid_qty,
            ask_qty,
            expected_edge_bps: width.max(0.0),
        })
    }
}

#[async_trait]
impl Strategy for MarketMakingStrategy {
    fn name(&self) -> &str {
        "market_making"
    }

    async fn evaluate(&self, snapshot: &MarketSnapshot) -> Result<Option<Vec<TradeSignal>>> {
        let mut signals = Vec::new();
        let mut last_quotes = self.last_quotes.lock().unwrap();

        for (symbol, book) in &snapshot.order_books {
            let inventory = snapshot.positions.get(symbol).copied().unwrap_or(0.0);
            let Some(quote) = (if self.engine == QuoteEngine::VolObi {
                self.quote_vol_obi(book, inventory)
                    .and_then(|q| {
                        apply_inventory_cap(q, inventory, self.params.max_inventory, self.inventory_mode)
                    })
            } else {
                quote_from_book(book, inventory, &self.params, self.inventory_mode)
            }) else {
                continue;
            };

            if let Some((prev_mid, prev_inv, prev_ts)) = last_quotes.get(symbol) {
                if !should_requote(
                    *prev_mid,
                    *prev_inv,
                    *prev_ts,
                    quote.mid,
                    inventory,
                    book.timestamp,
                    self.params.bid_spread,
                    self.params.min_requote_secs,
                ) {
                    continue;
                }
            }
            last_quotes.insert(symbol.clone(), (quote.mid, inventory, book.timestamp));

            let ts = if book.timestamp.timestamp() > 0 {
                book.timestamp
            } else {
                Utc::now()
            };

            if quote.bid_qty * quote.bid_price >= DUST_NOTIONAL {
                signals.push(TradeSignal {
                    symbol: symbol.clone(),
                    market_id: book.market_id,
                    side: Side::Buy,
                    price: quote.bid_price,
                    quantity: quote.bid_qty,
                    order_type: OrderType::Limit,
                    reason: format!(
                        "MM {} bid {:.6} mid={:.6} inv={:.6}",
                        if self.engine == QuoteEngine::VolObi {
                            "vol_obi"
                        } else {
                            "pmm"
                        },
                        quote.bid_price,
                        quote.mid,
                        inventory
                    ),
                    timestamp: ts,
                    expected_edge_bps: Some(quote.expected_edge_bps),
                    risk_reducing: inventory < 0.0
                        && quote.bid_qty <= inventory.abs() + f64::EPSILON,
                });
            }
            if quote.ask_qty * quote.ask_price >= DUST_NOTIONAL {
                signals.push(TradeSignal {
                    symbol: symbol.clone(),
                    market_id: book.market_id,
                    side: Side::Sell,
                    price: quote.ask_price,
                    quantity: quote.ask_qty,
                    order_type: OrderType::Limit,
                    reason: format!(
                        "MM {} ask {:.6} mid={:.6} inv={:.6}",
                        if self.engine == QuoteEngine::VolObi {
                            "vol_obi"
                        } else {
                            "pmm"
                        },
                        quote.ask_price,
                        quote.mid,
                        inventory
                    ),
                    timestamp: ts,
                    expected_edge_bps: Some(quote.expected_edge_bps),
                    risk_reducing: inventory > 0.0 && quote.ask_qty <= inventory + f64::EPSILON,
                });
            }
        }

        if signals.is_empty() {
            Ok(None)
        } else {
            debug!(count = signals.len(), "MM quotes");
            Ok(Some(signals))
        }
    }

    fn reset(&mut self) {
        self.last_quotes.lock().unwrap().clear();
        self.vol_engines.lock().unwrap().clear();
    }

    fn clear_filled_state(&self) {
        self.last_quotes.lock().unwrap().clear();
    }
}

#[cfg(test)]
#[path = "market_making_tests.rs"]
mod market_making_tests;
