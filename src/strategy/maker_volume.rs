//! Robinhood maker-volume gates.
//!
//! Split from the un-gated multi-name vol_obi spray: maker-only, BTC-first,
//! notional inventory cap, and a short markout backoff. Both enable flags
//! default false so checking out this tree cannot start quoting.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use config::Config;
use std::collections::HashMap;

use super::market_making::{mid_from_bbo, TwoSidedQuote};
use crate::lighter::types::Side;

/// Hard gates for a future RH maker-volume book. Defaults never arm quotes.
#[derive(Debug, Clone, PartialEq)]
pub struct MakerVolumeConfig {
    pub enabled: bool,
    pub allow_quotes: bool,
    pub markets: Vec<u32>,
    pub order_notional: f64,
    pub max_position_notional: f64,
    pub inventory_skew: f64,
    pub markout_window_secs: i64,
    pub markout_adverse_bps: f64,
    pub backoff_secs: i64,
    pub min_half_spread_bps: f64,
}

impl Default for MakerVolumeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_quotes: false,
            markets: vec![1],
            order_notional: 20.0,
            max_position_notional: 60.0,
            inventory_skew: 0.5,
            markout_window_secs: 3,
            markout_adverse_bps: 4.0,
            backoff_secs: 15,
            min_half_spread_bps: 2.0,
        }
    }
}

impl MakerVolumeConfig {
    pub fn quotes_armed(&self) -> bool {
        self.enabled && self.allow_quotes
    }

    pub fn allows_market(&self, market_id: u32) -> bool {
        self.markets.is_empty() || self.markets.contains(&market_id)
    }
}

/// True when the yaml actually declared the maker_volume section.
pub fn maker_volume_configured(settings: &Config) -> bool {
    settings
        .get_bool("trading.strategies.maker_volume.enabled")
        .is_ok()
        || settings
            .get_bool("trading.strategies.maker_volume.allow_quotes")
            .is_ok()
}

pub fn maker_volume_from_settings(settings: &Config) -> MakerVolumeConfig {
    let mut cfg = MakerVolumeConfig::default();
    if let Ok(v) = settings.get_bool("trading.strategies.maker_volume.enabled") {
        cfg.enabled = v;
    }
    if let Ok(v) = settings.get_bool("trading.strategies.maker_volume.allow_quotes") {
        cfg.allow_quotes = v;
    }
    if let Ok(v) = settings.get_float("trading.strategies.maker_volume.order_notional") {
        if v.is_finite() && v > 0.0 {
            cfg.order_notional = v;
        }
    }
    if let Ok(v) = settings.get_float("trading.strategies.maker_volume.max_position_notional") {
        if v.is_finite() && v > 0.0 {
            cfg.max_position_notional = v;
        }
    }
    if let Ok(v) = settings.get_float("trading.strategies.maker_volume.inventory_skew") {
        if v.is_finite() && v >= 0.0 {
            cfg.inventory_skew = v;
        }
    }
    if let Ok(v) = settings.get_int("trading.strategies.maker_volume.markout_window_secs") {
        cfg.markout_window_secs = v.max(1);
    }
    if let Ok(v) = settings.get_float("trading.strategies.maker_volume.markout_adverse_bps") {
        if v.is_finite() && v > 0.0 {
            cfg.markout_adverse_bps = v;
        }
    }
    if let Ok(v) = settings.get_int("trading.strategies.maker_volume.backoff_secs") {
        cfg.backoff_secs = v.max(1);
    }
    if let Ok(v) = settings.get_float("trading.strategies.maker_volume.min_half_spread_bps") {
        if v.is_finite() && v > 0.0 {
            cfg.min_half_spread_bps = v;
        }
    }
    if let Ok(arr) = settings.get_array("trading.strategies.maker_volume.markets") {
        let ids: Vec<u32> = arr
            .into_iter()
            .filter_map(|item| item.into_int().ok())
            .filter(|n| *n >= 0)
            .map(|n| n as u32)
            .collect();
        if !ids.is_empty() {
            cfg.markets = ids;
        }
    }
    cfg
}

/// Persisted dashboard `market_making` must not start quoting unless both
/// maker_volume flags are on. Checkout of this PR keeps them off.
pub fn refuse_persisted_mm(saved_name: &str, settings: &Config) -> bool {
    let is_mm =
        saved_name.eq_ignore_ascii_case("market_making") || saved_name.eq_ignore_ascii_case("mm");
    is_mm && !maker_volume_from_settings(settings).quotes_armed()
}

/// Drop any price that would cross or lock the spread (taker chase).
pub fn clamp_maker_only(
    quote: TwoSidedQuote,
    best_bid: f64,
    best_ask: f64,
) -> Option<TwoSidedQuote> {
    let mid = mid_from_bbo(best_bid, best_ask)?;
    let mut q = quote;
    q.mid = mid;
    if q.bid_qty > 0.0
        && !(q.bid_price.is_finite()
            && q.bid_price > 0.0
            && q.bid_price < best_ask
            && q.bid_price < mid)
    {
        q.bid_qty = 0.0;
    }
    if q.ask_qty > 0.0
        && !(q.ask_price.is_finite()
            && q.ask_price > 0.0
            && q.ask_price > best_bid
            && q.ask_price > mid)
    {
        q.ask_qty = 0.0;
    }
    if q.bid_qty > 0.0 && q.ask_qty > 0.0 && q.bid_price >= q.ask_price {
        return None;
    }
    if q.bid_qty <= 0.0 && q.ask_qty <= 0.0 {
        return None;
    }
    Some(q)
}

/// Hard notional cap: stop bidding when long-full, stop offering when short-full.
pub fn apply_notional_inventory_cap(
    quote: TwoSidedQuote,
    inventory_base: f64,
    mid: f64,
    max_position_notional: f64,
) -> Option<TwoSidedQuote> {
    if !max_position_notional.is_finite()
        || max_position_notional <= 0.0
        || !mid.is_finite()
        || mid <= 0.0
    {
        return Some(quote);
    }
    let notional = inventory_base * mid;
    let mut q = quote;
    if notional >= max_position_notional - 1e-9 {
        q.bid_qty = 0.0;
    }
    if notional <= -max_position_notional + 1e-9 {
        q.ask_qty = 0.0;
    }
    if q.bid_qty <= 0.0 && q.ask_qty <= 0.0 {
        return None;
    }
    Some(q)
}

#[derive(Clone, Copy, Debug)]
struct FillMark {
    side: Side,
    mid: f64,
    ts: DateTime<Utc>,
}

/// Inventory-delta markout: pull the filled side when mid goes against us.
#[derive(Debug, Default)]
pub struct MarkoutTracker {
    last_inv: HashMap<String, f64>,
    last_fill: HashMap<String, FillMark>,
    block_bid_until: HashMap<String, DateTime<Utc>>,
    block_ask_until: HashMap<String, DateTime<Utc>>,
}

impl MarkoutTracker {
    pub fn on_book(
        &mut self,
        symbol: &str,
        inventory: f64,
        mid: f64,
        now: DateTime<Utc>,
        cfg: &MakerVolumeConfig,
    ) {
        if let Some(prev) = self.last_inv.get(symbol).copied() {
            let delta = inventory - prev;
            if delta.abs() > 1e-12 && mid.is_finite() && mid > 0.0 {
                self.last_fill.insert(
                    symbol.to_string(),
                    FillMark {
                        side: if delta > 0.0 { Side::Buy } else { Side::Sell },
                        mid,
                        ts: now,
                    },
                );
            }
        }
        self.last_inv.insert(symbol.to_string(), inventory);

        let Some(fill) = self.last_fill.get(symbol).copied() else {
            return;
        };
        if !fill.mid.is_finite() || fill.mid <= 0.0 || !mid.is_finite() {
            return;
        }
        let window = ChronoDuration::seconds(cfg.markout_window_secs.max(1));
        if now.signed_duration_since(fill.ts) > window {
            return;
        }
        let move_bps = ((mid - fill.mid) / fill.mid) * 10_000.0;
        let adverse = match fill.side {
            Side::Buy => -move_bps,
            Side::Sell => move_bps,
        };
        if adverse + 1e-12 < cfg.markout_adverse_bps {
            return;
        }
        let until = now + ChronoDuration::seconds(cfg.backoff_secs.max(1));
        match fill.side {
            Side::Buy => {
                self.block_bid_until.insert(symbol.to_string(), until);
            }
            Side::Sell => {
                self.block_ask_until.insert(symbol.to_string(), until);
            }
        }
    }

    pub fn bid_blocked(&self, symbol: &str, now: DateTime<Utc>) -> bool {
        self.block_bid_until
            .get(symbol)
            .is_some_and(|until| now < *until)
    }

    pub fn ask_blocked(&self, symbol: &str, now: DateTime<Utc>) -> bool {
        self.block_ask_until
            .get(symbol)
            .is_some_and(|until| now < *until)
    }
}

pub fn apply_markout_pull(
    quote: TwoSidedQuote,
    symbol: &str,
    now: DateTime<Utc>,
    tracker: &MarkoutTracker,
) -> Option<TwoSidedQuote> {
    let mut q = quote;
    if tracker.bid_blocked(symbol, now) {
        q.bid_qty = 0.0;
    }
    if tracker.ask_blocked(symbol, now) {
        q.ask_qty = 0.0;
    }
    if q.bid_qty <= 0.0 && q.ask_qty <= 0.0 {
        return None;
    }
    Some(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn quote(bid: f64, ask: f64) -> TwoSidedQuote {
        TwoSidedQuote {
            mid: (bid + ask) / 2.0,
            bid_price: bid,
            ask_price: ask,
            bid_qty: 0.01,
            ask_qty: 0.01,
            expected_edge_bps: 10.0,
        }
    }

    #[test]
    fn defaults_are_disarmed_and_btc_only() {
        let cfg = MakerVolumeConfig::default();
        assert!(!cfg.enabled && !cfg.allow_quotes);
        assert!(!cfg.quotes_armed());
        assert_eq!(cfg.markets, vec![1]);
        assert!((cfg.order_notional - 20.0).abs() < 1e-12);
        assert!((cfg.max_position_notional - 60.0).abs() < 1e-12);
    }

    #[test]
    fn missing_yaml_keys_do_not_arm_quotes() {
        let settings = Config::builder().build().unwrap();
        let cfg = maker_volume_from_settings(&settings);
        assert!(!cfg.quotes_armed());
        assert!(!maker_volume_configured(&settings));
        assert!(refuse_persisted_mm("market_making", &settings));
        assert!(!refuse_persisted_mm("trend_following", &settings));
    }

    #[test]
    fn both_flags_required_to_arm() {
        let only_enabled = Config::builder()
            .set_override("trading.strategies.maker_volume.enabled", true)
            .unwrap()
            .build()
            .unwrap();
        assert!(!maker_volume_from_settings(&only_enabled).quotes_armed());

        let only_allow = Config::builder()
            .set_override("trading.strategies.maker_volume.allow_quotes", true)
            .unwrap()
            .build()
            .unwrap();
        assert!(!maker_volume_from_settings(&only_allow).quotes_armed());

        let armed = Config::builder()
            .set_override("trading.strategies.maker_volume.enabled", true)
            .unwrap()
            .set_override("trading.strategies.maker_volume.allow_quotes", true)
            .unwrap()
            .build()
            .unwrap();
        assert!(maker_volume_from_settings(&armed).quotes_armed());
        assert!(!refuse_persisted_mm("mm", &armed));
    }

    #[test]
    fn shipped_robinhood_yaml_does_not_arm_maker_volume() {
        let settings = Config::builder()
            .add_source(config::File::with_name("config/settings.robinhood.yaml"))
            .build()
            .expect("rh yaml");
        let cfg = maker_volume_from_settings(&settings);
        assert!(maker_volume_configured(&settings));
        assert!(!cfg.quotes_armed());
        assert!(!cfg.enabled && !cfg.allow_quotes);
        assert_eq!(cfg.markets, vec![1]);
        assert!(refuse_persisted_mm("market_making", &settings));
    }

    #[test]
    fn maker_only_rejects_crossed_and_through_mid_quotes() {
        let book_bid = 99.5;
        let book_ask = 100.5;
        assert!(
            clamp_maker_only(quote(100.6, 101.0), book_bid, book_ask).is_none()
                || clamp_maker_only(quote(100.6, 101.0), book_bid, book_ask)
                    .is_some_and(|q| q.bid_qty == 0.0)
        );
        let crossed = clamp_maker_only(quote(100.6, 100.4), book_bid, book_ask);
        assert!(crossed.is_none() || crossed.is_some_and(|q| q.bid_qty == 0.0 || q.ask_qty == 0.0));
        let locked_mid = clamp_maker_only(quote(100.0, 100.0), book_bid, book_ask);
        assert!(locked_mid.is_none(), "quotes through mid must not stand");
        let ok = clamp_maker_only(quote(99.4, 100.6), book_bid, book_ask).unwrap();
        assert!(ok.bid_qty > 0.0 && ok.ask_qty > 0.0);
        assert!(ok.bid_price < ok.mid && ok.mid < ok.ask_price);
    }

    #[test]
    fn inventory_notional_blocks_adding_side() {
        let q = quote(99.4, 100.6);
        let long_full = apply_notional_inventory_cap(q, 0.001, 70_000.0, 60.0).unwrap();
        assert_eq!(long_full.bid_qty, 0.0);
        assert!(long_full.ask_qty > 0.0);

        let short_full = apply_notional_inventory_cap(q, -0.001, 70_000.0, 60.0).unwrap();
        assert_eq!(short_full.ask_qty, 0.0);
        assert!(short_full.bid_qty > 0.0);

        let flat = apply_notional_inventory_cap(q, 0.0, 70_000.0, 60.0).unwrap();
        assert!(flat.bid_qty > 0.0 && flat.ask_qty > 0.0);
    }

    #[test]
    fn markout_pulls_filled_side_on_adverse_mid() {
        let mut tracker = MarkoutTracker::default();
        let cfg = MakerVolumeConfig {
            enabled: true,
            allow_quotes: true,
            markout_window_secs: 3,
            markout_adverse_bps: 4.0,
            backoff_secs: 15,
            ..MakerVolumeConfig::default()
        };
        let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        tracker.on_book("BTC", 0.0, 70_000.0, t0, &cfg);
        tracker.on_book("BTC", 0.0002, 70_000.0, t0, &cfg);
        let t1 = t0 + ChronoDuration::seconds(1);
        tracker.on_book("BTC", 0.0002, 70_000.0 * (1.0 - 0.0008), t1, &cfg);
        assert!(
            tracker.bid_blocked("BTC", t1),
            "buy fill + mid down must pull bids"
        );
        assert!(!tracker.ask_blocked("BTC", t1));
        let pulled = apply_markout_pull(quote(69_900.0, 70_100.0), "BTC", t1, &tracker).unwrap();
        assert_eq!(pulled.bid_qty, 0.0);
        assert!(pulled.ask_qty > 0.0);
        let t_later = t0 + ChronoDuration::seconds(20);
        assert!(!tracker.bid_blocked("BTC", t_later));
    }
}
