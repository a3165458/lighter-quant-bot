use std::time::Duration;

use crate::hft::{BboUpdate, BookHealth, StandardRateBudget};
use crate::lighter::types::MarketInfo;

/// Inputs the selector needs for one discovered market.
#[derive(Debug, Clone)]
pub struct UniverseMarket {
    pub market_id: u32,
    #[allow(dead_code)]
    pub symbol: String,
    pub market_type: String,
    pub bid_price: f64,
    pub ask_price: f64,
    pub bid_size: f64,
    pub ask_size: f64,
    pub book_health: BookHealth,
}

impl UniverseMarket {
    pub fn from_catalog_and_bbo(info: &MarketInfo, bbo: Option<&BboUpdate>) -> Self {
        let (bid_price, ask_price, bid_size, ask_size, book_health) = match bbo {
            Some(b) => (
                b.bid_price,
                b.ask_price,
                b.bid_size,
                b.ask_size,
                BookHealth::Live,
            ),
            None => (0.0, 0.0, 0.0, 0.0, BookHealth::Syncing),
        };
        Self {
            market_id: info.market_id,
            symbol: info.symbol.clone(),
            market_type: info.market_type.clone(),
            bid_price,
            ask_price,
            bid_size,
            ask_size,
            book_health,
        }
    }

    pub fn mid(&self) -> Option<f64> {
        if !self.bid_price.is_finite() || !self.ask_price.is_finite() {
            return None;
        }
        if self.bid_price <= 0.0 || self.ask_price <= 0.0 || self.ask_price < self.bid_price {
            return None;
        }
        let mid = (self.bid_price + self.ask_price) / 2.0;
        (mid > 0.0).then_some(mid)
    }

    pub fn spread_bps(&self) -> Option<f64> {
        let mid = self.mid()?;
        Some((self.ask_price - self.bid_price) / mid * 10_000.0)
    }
}

/// Filters applied after catalog discovery. No hardcoded market ids.
#[derive(Debug, Clone)]
pub struct UniverseSelectParams {
    pub max_spread_bps: f64,
    pub min_spread_bps: f64,
    pub min_size: f64,
    /// When true, rank wider (but still capped) spreads first — better maker fills.
    pub prefer_wider_spreads: bool,
    /// How often we intend to cancel-replace both sides on each quoted name.
    pub refresh_interval: Duration,
    /// Trading requests consumed per refresh (cancel+bid+ask ≈ 3; two-sided place ≈ 2).
    pub actions_per_refresh: u32,
}

impl Default for UniverseSelectParams {
    fn default() -> Self {
        Self {
            max_spread_bps: 25.0,
            min_spread_bps: 0.0,
            min_size: 0.01,
            prefer_wider_spreads: false,
            refresh_interval: Duration::from_secs(15),
            actions_per_refresh: 3,
        }
    }
}

/// How many names a Standard account can keep fresh at 1 action/sec.
///
/// Each quoted market consumes `actions_per_refresh` trading requests every
/// `refresh_interval`. The shipped [`StandardRateBudget`] is used to count
/// how many actions fit in that window.
pub fn max_markets_for_standard_budget(
    refresh_interval: Duration,
    actions_per_refresh: u32,
) -> usize {
    if actions_per_refresh == 0 || refresh_interval.is_zero() {
        return 0;
    }
    let mut budget = StandardRateBudget::new();
    let mut actions = 0u32;
    let mut now = Duration::ZERO;
    while now <= refresh_interval {
        if budget.try_acquire_at(now) {
            actions += 1;
        }
        now += Duration::from_secs(1);
    }
    (actions / actions_per_refresh) as usize
}

pub fn qualify_market(market: &UniverseMarket, params: &UniverseSelectParams) -> bool {
    if !market.market_type.eq_ignore_ascii_case("perp") {
        return false;
    }
    if market.book_health != BookHealth::Live {
        return false;
    }
    let Some(mid) = market.mid() else {
        return false;
    };
    if mid <= 0.0 {
        return false;
    }
    if market.bid_size < params.min_size || market.ask_size < params.min_size {
        return false;
    }
    match market.spread_bps() {
        Some(spread)
            if spread <= params.max_spread_bps && spread >= params.min_spread_bps =>
        {
            true
        }
        _ => false,
    }
}

/// Rank qualified perps (tighter spread, then deeper size) and cap to the
/// Standard one-action-per-second refresh budget.
pub fn select_quoting_universe(
    catalog: &[UniverseMarket],
    params: &UniverseSelectParams,
) -> Vec<UniverseMarket> {
    let cap = max_markets_for_standard_budget(params.refresh_interval, params.actions_per_refresh);
    if cap == 0 {
        return Vec::new();
    }

    let mut qualified: Vec<UniverseMarket> = catalog
        .iter()
        .filter(|m| qualify_market(m, params))
        .cloned()
        .collect();

    qualified.sort_by(|left, right| {
        let ls = left.spread_bps().unwrap_or(f64::MAX);
        let rs = right.spread_bps().unwrap_or(f64::MAX);
        let spread_ord = if params.prefer_wider_spreads {
            rs.total_cmp(&ls)
        } else {
            ls.total_cmp(&rs)
        };
        spread_ord
            .then_with(|| {
                let lsz = left.bid_size.min(left.ask_size);
                let rsz = right.bid_size.min(right.ask_size);
                rsz.total_cmp(&lsz)
            })
            .then_with(|| left.market_id.cmp(&right.market_id))
    });
    qualified.truncate(cap);
    qualified
}

/// Join an exchange catalog with the latest BBO map. Spot names stay in the
/// catalog so the selector can reject them; they are never quoted.
pub fn markets_from_catalog(
    infos: &[MarketInfo],
    bbos: &[(u32, BboUpdate)],
) -> Vec<UniverseMarket> {
    infos
        .iter()
        .map(|info| {
            let bbo = bbos
                .iter()
                .find(|(id, _)| *id == info.market_id)
                .map(|(_, b)| b);
            UniverseMarket::from_catalog_and_bbo(info, bbo)
        })
        .collect()
}

/// Discovered perp ids in catalog order. Used to *observe* every perp; quoting
/// still goes through [`resolve_quoting_market_ids`].
pub fn discover_perp_ids(catalog: &[MarketInfo]) -> Vec<u32> {
    let mut ids: Vec<u32> = catalog
        .iter()
        .filter(|market| market.market_type.eq_ignore_ascii_case("perp"))
        .map(|market| market.market_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Rank/filter a quoting set. Empty or incomplete books yield an empty set —
/// callers must not treat that as "quote the first N perp ids".
pub fn resolve_quoting_market_ids(
    catalog: &[MarketInfo],
    bbos: &[(u32, BboUpdate)],
    params: &UniverseSelectParams,
) -> Vec<u32> {
    select_quoting_universe(&markets_from_catalog(catalog, bbos), params)
        .into_iter()
        .map(|market| market.market_id)
        .collect()
}

/// Live auto/explicit universe. `subscribe_ids` may include every discovered
/// perp so BBO can arrive; `quoting_ids` is empty until books exist and pass
/// qualify/rank. Never freezes on first-N-by-id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveUniverse {
    pub subscribe_ids: Vec<u32>,
    pub quoting_ids: Vec<u32>,
    pub awaiting_books: bool,
}

pub fn resolve_live_universe(
    mode: &str,
    configured: &[u32],
    catalog: &[MarketInfo],
    bbos: &[(u32, BboUpdate)],
    params: &UniverseSelectParams,
) -> LiveUniverse {
    let mode = mode.trim().to_ascii_lowercase();
    if mode != "auto" && mode != "all" {
        return LiveUniverse {
            subscribe_ids: configured.to_vec(),
            quoting_ids: configured.to_vec(),
            awaiting_books: false,
        };
    }
    if catalog.is_empty() {
        return LiveUniverse {
            subscribe_ids: configured.to_vec(),
            quoting_ids: configured.to_vec(),
            awaiting_books: false,
        };
    }
    let subscribe_ids = discover_perp_ids(catalog);
    let quoting_ids = resolve_quoting_market_ids(catalog, bbos, params);
    let awaiting_books = bbos.is_empty() || quoting_ids.is_empty();
    LiveUniverse {
        subscribe_ids,
        quoting_ids,
        awaiting_books,
    }
}

/// Build a one-level book from a ticker BBO so the strategy evaluate path
/// can quote without a full order-book channel.
pub fn order_book_from_bbo(bbo: &BboUpdate) -> crate::lighter::types::OrderBook {
    use chrono::{TimeZone, Utc};
    use crate::lighter::types::{OrderBook, PriceLevel};
    OrderBook {
        symbol: if bbo.symbol.is_empty() {
            crate::lighter::symbols::symbol_of(bbo.market_id)
        } else {
            bbo.symbol.clone()
        },
        market_id: bbo.market_id,
        bids: vec![PriceLevel {
            price: bbo.bid_price,
            quantity: bbo.bid_size,
        }],
        asks: vec![PriceLevel {
            price: bbo.ask_price,
            quantity: bbo.ask_size,
        }],
        timestamp: Utc
            .timestamp_millis_opt(bbo.exchange_timestamp_ms as i64)
            .single()
            .unwrap_or_else(Utc::now),
    }
}

pub fn bbo_from_order_book(book: &crate::lighter::types::OrderBook) -> Option<BboUpdate> {
    let bid = book.bids.first()?;
    let ask = book.asks.first()?;
    Some(BboUpdate {
        market_id: book.market_id,
        symbol: book.symbol.clone(),
        nonce: 0,
        exchange_timestamp_ms: book.timestamp.timestamp_millis().max(0) as u64,
        bid_price: bid.price,
        bid_size: bid.quantity,
        ask_price: ask.price,
        ask_size: ask.quantity,
    })
}
