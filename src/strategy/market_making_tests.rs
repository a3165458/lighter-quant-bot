use super::*;
use crate::lighter::types::{MarketSnapshot, OrderBook, OrderType, PriceLevel, Side};
use chrono::{TimeZone, Utc};

fn params() -> MmQuoteParams {
    MmQuoteParams {
        bid_spread: 0.001,
        ask_spread: 0.001,
        order_notional: 100.0,
        inventory_skew: 0.5,
        inventory_target: 2.0,
        max_inventory: 8.0,
        min_requote_secs: 10,
    }
}

fn book(symbol: &str, market_id: u32, bid: f64, ask: f64, bid_sz: f64, ask_sz: f64) -> OrderBook {
    OrderBook {
        symbol: symbol.to_string(),
        market_id,
        bids: vec![PriceLevel {
            price: bid,
            quantity: bid_sz,
        }],
        asks: vec![PriceLevel {
            price: ask,
            quantity: ask_sz,
        }],
        timestamp: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn snapshot_from_book(book: OrderBook, inventory: f64) -> MarketSnapshot {
    let symbol = book.symbol.clone();
    let mut snap = MarketSnapshot::default();
    snap.order_books.insert(symbol.clone(), book);
    if inventory != 0.0 {
        snap.positions.insert(symbol, inventory);
    }
    snap
}

#[test]
fn join_bbo_when_book_is_inside_configured_offset() {
    let mut p = params();
    p.bid_spread = 0.002;
    p.ask_spread = 0.002;
    p.inventory_skew = 0.0;
    // 10 bps book, 20 bps configured floor → join the live touch.
    let q = quote_two_sided(99.95, 100.05, 0.0, &p).unwrap();
    assert!((q.bid_price - 99.95).abs() < 1e-9);
    assert!((q.ask_price - 100.05).abs() < 1e-9);
}

#[test]
fn two_sided_quotes_sit_strictly_around_mid() {
    let bid = 99.5;
    let ask = 100.5;
    let quote = quote_two_sided(bid, ask, 0.0, &params()).expect("valid book");
    let mid = (bid + ask) / 2.0;
    assert!(quote.bid_price < mid, "maker bid must be below mid");
    assert!(mid < quote.ask_price, "maker ask must be above mid");
    assert!(quote.bid_qty > 0.0 && quote.ask_qty > 0.0);
    assert!(quote.expected_edge_bps > 0.0);
}

#[test]
fn long_inventory_makes_ask_more_aggressive_and_bid_less() {
    let bid = 99.5;
    let ask = 100.5;
    let flat = quote_two_sided(bid, ask, 0.0, &params()).unwrap();
    let long = quote_two_sided(bid, ask, 2.0, &params()).unwrap();
    let short = quote_two_sided(bid, ask, -2.0, &params()).unwrap();

    assert!(
        long.ask_price < flat.ask_price,
        "long inventory must pull the ask toward mid (more aggressive sell)"
    );
    assert!(
        long.bid_price < flat.bid_price,
        "long inventory must push the bid away from mid (less aggressive buy)"
    );
    assert!(
        long.ask_qty > long.bid_qty,
        "long inventory must offer more size on the ask than the bid"
    );

    assert!(
        short.bid_price > flat.bid_price,
        "short inventory must pull the bid toward mid (more aggressive buy)"
    );
    assert!(
        short.ask_price > flat.ask_price,
        "short inventory must push the ask away from mid (less aggressive sell)"
    );
    assert!(
        short.bid_qty > short.ask_qty,
        "short inventory must offer more size on the bid than the ask"
    );
}

#[test]
fn invalid_books_produce_no_quotes() {
    let p = params();
    assert!(quote_two_sided(0.0, 100.0, 0.0, &p).is_none());
    assert!(quote_two_sided(-1.0, 100.0, 0.0, &p).is_none());
    assert!(quote_two_sided(100.0, 0.0, 0.0, &p).is_none());
    assert!(quote_two_sided(101.0, 100.0, 0.0, &p).is_none());
    assert!(quote_two_sided(f64::NAN, 100.0, 0.0, &p).is_none());
    assert!(mid_from_bbo(50.0, 49.0).is_none());
}

#[test]
fn hard_cap_drops_same_side_adding_quote() {
    let p = params();
    let quote = quote_two_sided(99.5, 100.5, 8.0, &p).unwrap();
    let capped = apply_inventory_cap(quote, 8.0, p.max_inventory, InventoryMode::Hard).unwrap();
    assert_eq!(capped.bid_qty, 0.0, "long at hard cap must not add more");
    assert!(capped.ask_qty > 0.0, "reducing ask must remain");
}

#[tokio::test]
async fn evaluate_does_not_stack_quotes_on_the_next_tick() {
    let strategy = MarketMakingStrategy::new(params(), InventoryMode::Hard).unwrap();
    let first = strategy
        .evaluate(&snapshot_from_book(
            book("BTC", 1, 99.5, 100.5, 2.0, 2.0),
            0.0,
        ))
        .await
        .unwrap();
    assert!(first.is_some());
    let mut later = book("BTC", 1, 99.52, 100.52, 2.0, 2.0);
    later.timestamp = Utc.timestamp_opt(1_700_000_003, 0).unwrap();
    let stacked = strategy
        .evaluate(&snapshot_from_book(later, 0.0))
        .await
        .unwrap();
    assert!(
        stacked.is_none(),
        "same book a few seconds later must not emit another bid/ask pair"
    );
}

#[tokio::test]
async fn evaluate_emits_maker_limit_signals_with_edge() {
    let strategy = MarketMakingStrategy::new(params(), InventoryMode::Hard).unwrap();
    let snap = snapshot_from_book(book("BTC", 1, 99.5, 100.5, 2.0, 2.0), 0.0);
    let signals = strategy
        .evaluate(&snap)
        .await
        .unwrap()
        .expect("two-sided quotes");
    assert_eq!(signals.len(), 2);
    assert!(signals.iter().all(|s| s.order_type == OrderType::Limit));
    assert!(signals
        .iter()
        .all(|s| s.expected_edge_bps.unwrap_or(0.0) > 0.0));
    let buy = signals.iter().find(|s| s.side == Side::Buy).unwrap();
    let sell = signals.iter().find(|s| s.side == Side::Sell).unwrap();
    let mid = (99.5 + 100.5) / 2.0;
    assert!(buy.price < mid && mid < sell.price);
}

#[test]
fn requote_is_blocked_inside_min_interval_even_if_mid_moves() {
    let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let t1 = t0 + chrono::Duration::seconds(3);
    assert!(
        !should_requote(100.0, 0.0, t0, 100.2, 0.0, t1, 0.001, 10),
        "must not stack quotes every few seconds"
    );
}

#[test]
fn requote_after_interval_requires_meaningful_mid_move() {
    let t0 = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let t1 = t0 + chrono::Duration::seconds(10);
    assert!(
        !should_requote(100.0, 0.0, t0, 100.001, 0.0, t1, 0.001, 10),
        "0.1 bps tick is not enough to replace"
    );
    assert!(should_requote(100.0, 0.0, t0, 100.05, 0.0, t1, 0.001, 10));
}

#[tokio::test]
async fn evaluate_skips_crossed_book() {
    let strategy = MarketMakingStrategy::new(params(), InventoryMode::Hard).unwrap();
    let snap = snapshot_from_book(book("BTC", 1, 101.0, 100.0, 2.0, 2.0), 0.0);
    assert!(strategy.evaluate(&snap).await.unwrap().is_none());
}

#[tokio::test]
async fn mm_limit_signals_are_visible_to_profitability_gate() {
    use crate::risk::profitability::{ProfitabilityGuard, SignalEconomics};
    use config::Config;

    let strategy = MarketMakingStrategy::new(params(), InventoryMode::Hard).unwrap();
    let snap = snapshot_from_book(book("BTC", 1, 99.5, 100.5, 2.0, 2.0), 0.0);
    let signals = strategy.evaluate(&snap).await.unwrap().expect("quotes");
    let guard = ProfitabilityGuard::from_config(
        &Config::builder()
            .set_override("profitability.enabled", true)
            .unwrap()
            .set_override("profitability.entry_slippage_bps", 2.0)
            .unwrap()
            .set_override("profitability.exit_slippage_bps", 2.0)
            .unwrap()
            .set_override("profitability.adverse_selection_bps", 3.0)
            .unwrap()
            .set_override("profitability.min_net_edge_bps", 2.0)
            .unwrap()
            .build()
            .unwrap(),
    )
    .unwrap();
    for sig in &signals {
        assert_eq!(sig.order_type, OrderType::Limit);
        let decision = guard.evaluate(SignalEconomics::entry(sig.expected_edge_bps));
        assert!(
            decision.allowed,
            "MM quoted edge must be evaluable by the existing gate: {:?}",
            decision
        );
    }
}

fn warm_vol_strategy() -> MarketMakingStrategy {
    let mut p = params();
    p.min_requote_secs = 1;
    let mut settings = VolObiMmSettings::default();
    settings.alpha_source = AlphaSource::Local;
    settings.vol_obi.min_warmup_samples = 8;
    settings.vol_obi.window_steps = 64;
    settings.vol_obi.c1_ticks = 0.0;
    settings.vol_obi.skew = 0.0;
    settings.inventory_exit.enabled = false;
    MarketMakingStrategy::with_engine(p, InventoryMode::Hard, QuoteEngine::VolObi, settings)
        .unwrap()
}

#[tokio::test]
async fn vol_obi_quotes_after_warmup_sit_outside_mid() {
    let strategy = warm_vol_strategy();
    let mut last = None;
    for i in 0..16 {
        let mut b = book(
            "BTC",
            1,
            99.5 + i as f64 * 0.01,
            100.5 + i as f64 * 0.01,
            2.0,
            2.0,
        );
        b.timestamp = Utc.timestamp_opt(1_700_000_000 + i, 0).unwrap();
        if let Some(signals) = strategy
            .evaluate(&snapshot_from_book(b, 0.0))
            .await
            .unwrap()
        {
            last = Some(signals);
        }
    }
    let signals = last.expect("warmed vol_obi quotes");
    assert_eq!(signals.len(), 2);
    let buy = signals.iter().find(|s| s.side == Side::Buy).unwrap();
    let sell = signals.iter().find(|s| s.side == Side::Sell).unwrap();
    let mid = (99.5 + 15.0 * 0.01 + 100.5 + 15.0 * 0.01) / 2.0;
    assert!(buy.price < mid && mid < sell.price);
    assert!(buy.expected_edge_bps.unwrap() + 1e-6 >= 4.0);
}

#[tokio::test]
async fn vol_obi_does_not_join_tight_bbo() {
    let strategy = warm_vol_strategy();
    let mut last = None;
    for i in 0..16 {
        // 2 bps book — simple PMM would join; vol_obi stays at its half-spread floor.
        let mut b = book(
            "ETH",
            0,
            1999.8 + i as f64 * 0.01,
            2000.2 + i as f64 * 0.01,
            1.0,
            1.0,
        );
        b.timestamp = Utc.timestamp_opt(1_700_000_000 + i, 0).unwrap();
        if let Some(signals) = strategy
            .evaluate(&snapshot_from_book(b, 0.0))
            .await
            .unwrap()
        {
            last = Some(signals);
        }
    }
    let signals = last.expect("quotes");
    let buy = signals.iter().find(|s| s.side == Side::Buy).unwrap();
    let sell = signals.iter().find(|s| s.side == Side::Sell).unwrap();
    assert!(buy.price < 1999.8, "must not join the live bid");
    assert!(sell.price > 2000.2, "must not join the live ask");
}

#[test]
fn quote_engine_parse() {
    assert_eq!(QuoteEngine::parse("vol_obi").unwrap(), QuoteEngine::VolObi);
    assert_eq!(QuoteEngine::parse("simple").unwrap(), QuoteEngine::Simple);
    assert!(QuoteEngine::parse("avellaneda").is_err());
}

#[tokio::test]
async fn maker_volume_disarmed_emits_no_quotes() {
    let strategy = MarketMakingStrategy::new(params(), InventoryMode::Hard)
        .unwrap()
        .with_maker_volume(super::super::maker_volume::MakerVolumeConfig::default());
    let sigs = strategy
        .evaluate(&snapshot_from_book(
            book("BTC", 1, 99.5, 100.5, 2.0, 2.0),
            0.0,
        ))
        .await
        .unwrap();
    assert!(sigs.is_none(), "default flags must not start quoting");
}

#[tokio::test]
async fn maker_volume_armed_skips_non_btc_and_blocks_long_full() {
    let mut gates = super::super::maker_volume::MakerVolumeConfig::default();
    gates.enabled = true;
    gates.allow_quotes = true;
    gates.max_position_notional = 60.0;
    let strategy = MarketMakingStrategy::new(params(), InventoryMode::Hard)
        .unwrap()
        .with_maker_volume(gates);
    let eth = strategy
        .evaluate(&snapshot_from_book(
            book("ETH", 0, 3499.0, 3501.0, 1.0, 1.0),
            0.0,
        ))
        .await
        .unwrap();
    assert!(eth.is_none(), "BTC-only gate must skip ETH");

    let long_full = strategy
        .evaluate(&snapshot_from_book(
            book("BTC", 1, 69_950.0, 70_050.0, 1.0, 1.0),
            0.001,
        ))
        .await
        .unwrap()
        .expect("reducing ask");
    assert!(long_full.iter().all(|s| s.side != Side::Buy));
    assert!(long_full.iter().any(|s| s.side == Side::Sell));
}
