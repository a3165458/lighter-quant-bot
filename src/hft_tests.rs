use std::time::Duration;

use clap::Parser;

use crate::hft::{
    parse_bbo_update, plan_subscription_shards, resolve_live_universe, resolve_quoting_market_ids,
    select_quoting_universe, BboUpdate, BookContinuity, BookHealth, ScanStats, StandardRateBudget,
    UniverseMarket, UniverseSelectParams,
};
use crate::lighter::types::MarketInfo;
use crate::lighter::{types::WsMessage, websocket::LighterWebSocket};
use crate::{Cli, Commands};

#[test]
fn subscription_plan_respects_per_connection_limit() {
    let market_ids = (0..205).collect::<Vec<_>>();

    let shards = plan_subscription_shards(&market_ids, 100).expect("valid shard plan");

    assert_eq!(shards.len(), 3);
    assert_eq!(shards[0].len(), 100);
    assert_eq!(shards[1].len(), 100);
    assert_eq!(shards[2].len(), 5);
    assert_eq!(shards.into_iter().flatten().collect::<Vec<_>>(), market_ids);
}

#[test]
fn subscription_plan_rejects_zero_capacity() {
    let error = plan_subscription_shards(&[1, 2], 0).expect_err("zero capacity must fail");

    assert!(error.to_string().contains("greater than zero"));
}

#[test]
fn standard_rate_budget_allows_at_most_one_action_per_second() {
    let mut budget = StandardRateBudget::new();

    assert!(budget.try_acquire_at(Duration::ZERO));
    assert!(!budget.try_acquire_at(Duration::from_millis(999)));
    assert!(budget.try_acquire_at(Duration::from_secs(1)));
    assert!(!budget.try_acquire_at(Duration::from_millis(1_999)));
    assert!(budget.try_acquire_at(Duration::from_secs(2)));
}

#[test]
fn standard_rate_budget_does_not_burst_after_idle_time() {
    let mut budget = StandardRateBudget::new();

    assert!(budget.try_acquire_at(Duration::from_secs(120)));
    assert!(!budget.try_acquire_at(Duration::from_secs(120)));
}

#[test]
fn book_continuity_accepts_snapshot_and_contiguous_delta() {
    let mut continuity = BookContinuity::new();

    assert_eq!(continuity.apply_snapshot(10), BookHealth::Live);
    assert_eq!(continuity.apply_delta(10, 14), BookHealth::Live);
    assert_eq!(continuity.last_nonce(), Some(14));
}

#[test]
fn book_continuity_halts_on_nonce_gap_until_new_snapshot() {
    let mut continuity = BookContinuity::new();
    continuity.apply_snapshot(10);

    assert_eq!(continuity.apply_delta(11, 15), BookHealth::Halted);
    assert_eq!(continuity.apply_delta(15, 16), BookHealth::Halted);
    assert_eq!(continuity.apply_snapshot(20), BookHealth::Live);
    assert_eq!(continuity.last_nonce(), Some(20));
}

#[test]
fn parses_official_ticker_bbo_shape() {
    let message = serde_json::json!({
        "channel": "ticker:17",
        "nonce": 6442420597_u64,
        "ticker": {
            "s": "SOL",
            "a": {"price": "215.10", "size": "4.65"},
            "b": {"price": "214.99", "size": "17.45"}
        },
        "timestamp": 1773158679717_u64,
        "type": "update/ticker"
    });

    let bbo = parse_bbo_update(&message).expect("valid ticker");

    assert_eq!(bbo.market_id, 17);
    assert_eq!(bbo.symbol, "SOL");
    assert_eq!(bbo.nonce, 6_442_420_597);
    assert_eq!(bbo.exchange_timestamp_ms, 1_773_158_679_717);
    assert_eq!(bbo.bid_price, 214.99);
    assert_eq!(bbo.bid_size, 17.45);
    assert_eq!(bbo.ask_price, 215.10);
    assert_eq!(bbo.ask_size, 4.65);
}

#[test]
fn rejects_ticker_without_two_sided_positive_prices() {
    let missing_ask = serde_json::json!({
        "channel": "ticker:1",
        "nonce": 2,
        "ticker": {
            "s": "BTC",
            "a": {"price": "0", "size": "1"},
            "b": {"price": "100", "size": "1"}
        },
        "timestamp": 3,
        "type": "update/ticker"
    });

    assert!(parse_bbo_update(&missing_ask).is_err());
}

#[test]
fn websocket_emits_typed_bbo_updates() {
    let raw = serde_json::json!({
        "channel": "ticker:1",
        "nonce": 42,
        "ticker": {
            "s": "BTC",
            "a": {"price": "100.1", "size": "2"},
            "b": {"price": "100.0", "size": "3"}
        },
        "timestamp": 1773158679717_u64,
        "type": "update/ticker"
    })
    .to_string();

    let message = LighterWebSocket::parse_message(&raw)
        .expect("valid websocket envelope")
        .expect("data message");

    match message {
        WsMessage::BboUpdate(bbo) => {
            assert_eq!(bbo.market_id, 1);
            assert_eq!(bbo.nonce, 42);
        }
        other => panic!("expected BBO update, received {other:?}"),
    }
}

#[test]
fn ticker_subscription_uses_one_channel_per_market() {
    let message = LighterWebSocket::ticker_subscription_message(7);

    assert_eq!(
        message,
        serde_json::json!({
            "type": "subscribe",
            "channel": "ticker/7"
        })
    );
}

#[test]
fn scan_stats_report_event_rate_and_rank_current_spreads() {
    let mut stats = ScanStats::new();
    stats.record(BboUpdate {
        market_id: 1,
        symbol: "BTC".to_string(),
        nonce: 1,
        exchange_timestamp_ms: 1,
        bid_price: 100.0,
        bid_size: 2.0,
        ask_price: 100.1,
        ask_size: 2.0,
    });
    stats.record(BboUpdate {
        market_id: 2,
        symbol: "ETH".to_string(),
        nonce: 1,
        exchange_timestamp_ms: 1,
        bid_price: 50.0,
        bid_size: 2.0,
        ask_price: 50.2,
        ask_size: 2.0,
    });
    stats.record(BboUpdate {
        market_id: 1,
        symbol: "BTC".to_string(),
        nonce: 2,
        exchange_timestamp_ms: 2,
        bid_price: 100.0,
        bid_size: 2.0,
        ask_price: 100.2,
        ask_size: 2.0,
    });

    let summary = stats.summary(Duration::from_millis(100), 2);

    assert_eq!(summary.events, 3);
    assert_eq!(summary.live_markets, 2);
    assert_eq!(summary.events_per_second, 30.0);
    assert_eq!(summary.top_spreads.len(), 2);
    assert_eq!(summary.top_spreads[0].symbol, "ETH");
    assert!(summary.top_spreads[0].spread_bps > summary.top_spreads[1].spread_bps);
}

#[test]
fn scan_stats_can_reset_after_subscription_warmup() {
    let mut stats = ScanStats::new();
    stats.record(BboUpdate {
        market_id: 1,
        symbol: "BTC".to_string(),
        nonce: 1,
        exchange_timestamp_ms: 1,
        bid_price: 100.0,
        bid_size: 1.0,
        ask_price: 100.1,
        ask_size: 1.0,
    });

    stats.reset();
    let summary = stats.summary(Duration::from_secs(1), 10);

    assert_eq!(summary.events, 0);
    assert_eq!(summary.live_markets, 0);
    assert!(summary.top_spreads.is_empty());
}

#[test]
fn scan_cli_defaults_to_all_mainnet_markets_in_observation_mode() {
    let cli = Cli::try_parse_from(["lighter-bot", "scan"]).expect("valid scan command");

    match cli.command {
        Commands::Scan {
            url,
            ws_url,
            duration,
            top,
            market_type,
        } => {
            assert_eq!(url, "https://mainnet.zklighter.elliot.ai");
            assert_eq!(ws_url, "wss://mainnet.zklighter.elliot.ai/stream");
            assert_eq!(duration, 30);
            assert_eq!(top, 10);
            assert_eq!(market_type, "all");
        }
        _ => panic!("expected scan command"),
    }
}

fn fixture_info(id: u32, symbol: &str, market_type: &str) -> MarketInfo {
    MarketInfo {
        market_id: id,
        symbol: symbol.to_string(),
        size_decimals: 4,
        price_decimals: 2,
        min_base_amount: 0.001,
        min_quote_amount: 1.0,
        last_trade_price: 100.0,
        market_type: market_type.to_string(),
    }
}

fn fixture_market(
    id: u32,
    symbol: &str,
    market_type: &str,
    bid: f64,
    ask: f64,
    bid_sz: f64,
    ask_sz: f64,
    health: BookHealth,
) -> UniverseMarket {
    UniverseMarket {
        market_id: id,
        symbol: symbol.to_string(),
        market_type: market_type.to_string(),
        bid_price: bid,
        ask_price: ask,
        bid_size: bid_sz,
        ask_size: ask_sz,
        book_health: health,
    }
}

#[test]
fn universe_selector_keeps_healthy_perps_and_respects_standard_budget() {
    let params = UniverseSelectParams {
        max_spread_bps: 20.0,
        min_spread_bps: 0.0,
        min_size: 1.0,
        prefer_wider_spreads: false,
        refresh_interval: Duration::from_secs(10),
        actions_per_refresh: 2,
    };
    let cap = crate::hft::max_markets_for_standard_budget(
        params.refresh_interval,
        params.actions_per_refresh,
    );
    assert!(cap > 0, "10s window at 1 action/s must allow at least one name");

    let catalog = vec![
        fixture_market(1, "BTC", "perp", 100.0, 100.05, 5.0, 5.0, BookHealth::Live),
        fixture_market(2, "ETH", "perp", 50.0, 50.04, 8.0, 8.0, BookHealth::Live),
        fixture_market(3, "WIDE", "perp", 10.0, 10.10, 4.0, 4.0, BookHealth::Live),
        fixture_market(4, "THIN", "perp", 20.0, 20.01, 0.1, 0.1, BookHealth::Live),
        fixture_market(5, "SPOTY", "spot", 15.0, 15.01, 9.0, 9.0, BookHealth::Live),
        fixture_market(6, "HALT", "perp", 30.0, 30.01, 3.0, 3.0, BookHealth::Halted),
        fixture_market(7, "XED", "perp", 40.0, 39.00, 3.0, 3.0, BookHealth::Live),
        fixture_market(8, "SOL", "perp", 25.0, 25.02, 6.0, 6.0, BookHealth::Live),
        fixture_market(9, "AAPL", "perp", 180.0, 180.06, 2.0, 2.0, BookHealth::Live),
        fixture_market(10, "NVDA", "perp", 90.0, 90.04, 3.0, 3.0, BookHealth::Live),
        fixture_market(11, "MSFT", "perp", 70.0, 70.03, 3.0, 3.0, BookHealth::Live),
        fixture_market(12, "TSLA", "perp", 40.0, 40.03, 3.0, 3.0, BookHealth::Live),
        fixture_market(13, "META", "perp", 60.0, 60.04, 3.0, 3.0, BookHealth::Live),
        fixture_market(14, "AMD", "perp", 12.0, 12.01, 4.0, 4.0, BookHealth::Live),
    ];

    let selected = select_quoting_universe(&catalog, &params);
    assert!(
        selected.len() <= cap,
        "selector must not exceed Standard refresh budget ({cap})"
    );
    assert!(!selected.is_empty());
    let names: Vec<&str> = selected.iter().map(|m| m.symbol.as_str()).collect();
    assert!(
        !names.contains(&"WIDE"),
        "wide-spread perp must stay out: {names:?}"
    );
    assert!(
        !names.contains(&"THIN"),
        "thin book must stay out: {names:?}"
    );
    assert!(
        !names.contains(&"SPOTY"),
        "spot must stay out of the live universe: {names:?}"
    );
    assert!(
        !names.contains(&"HALT") && !names.contains(&"XED"),
        "halted/crossed books must stay out: {names:?}"
    );
    assert!(
        names.iter().all(|n| *n != "SPOTY"),
        "selector must never quote spot"
    );
    let shards = plan_subscription_shards(
        &selected.iter().map(|m| m.market_id).collect::<Vec<_>>(),
        100,
    )
    .unwrap();
    assert_eq!(shards.iter().map(|s| s.len()).sum::<usize>(), selected.len());
}

#[test]
fn resolve_quoting_ids_uses_catalog_not_hardcoded_ids() {
    let catalog = vec![
        fixture_info(16, "TSLA", "perp"),
        fixture_info(2048, "ETH/USDG", "spot"),
        fixture_info(23, "COIN", "perp"),
        fixture_info(1, "BTC", "perp"),
    ];
    let bbo = |id, bid, ask, sz| {
        (
            id,
            BboUpdate {
                market_id: id,
                symbol: String::new(),
                nonce: 1,
                exchange_timestamp_ms: 1,
                bid_price: bid,
                bid_size: sz,
                ask_price: ask,
                ask_size: sz,
            },
        )
    };
    let bbos = vec![
        bbo(16, 100.0, 100.02, 5.0),
        bbo(2048, 2000.0, 2000.10, 9.0),
        bbo(23, 50.0, 50.01, 4.0),
        bbo(1, 80.0, 80.02, 3.0),
    ];
    let params = UniverseSelectParams {
        max_spread_bps: 20.0,
        min_spread_bps: 0.0,
        min_size: 1.0,
        prefer_wider_spreads: false,
        refresh_interval: Duration::from_secs(10),
        actions_per_refresh: 2,
    };
    let ids = resolve_quoting_market_ids(&catalog, &bbos, &params);
    assert!(!ids.is_empty());
    assert!(!ids.contains(&2048), "spot id must not be selected");
    assert!(ids.iter().all(|id| catalog.iter().any(|m| m.market_id == *id)));
}

#[test]
fn live_resolver_empty_bbos_do_not_skip_qualify_rank() {
    // Lowest market_id is a wide-spread perp. A first-N-by-id fallback would
    // quote it forever when the live path passes &[] for books.
    let catalog = vec![
        fixture_info(1, "WIDE", "perp"),
        fixture_info(2, "THIN", "perp"),
        fixture_info(3, "SPOTY", "spot"),
        fixture_info(9, "TIGHT", "perp"),
    ];
    let params = UniverseSelectParams {
        max_spread_bps: 20.0,
        min_spread_bps: 0.0,
        min_size: 1.0,
        prefer_wider_spreads: false,
        refresh_interval: Duration::from_secs(10),
        actions_per_refresh: 2,
    };
    let empty = resolve_live_universe("auto", &[1], &catalog, &[], &params);
    assert!(
        empty.quoting_ids.is_empty(),
        "empty BBO must not produce a quoting set (got {:?})",
        empty.quoting_ids
    );
    assert!(
        empty.awaiting_books,
        "live resolver must keep waiting for books instead of freezing first-N ids"
    );
    assert!(
        empty.subscribe_ids.contains(&1) && empty.subscribe_ids.contains(&9),
        "auto mode still observes every discovered perp: {:?}",
        empty.subscribe_ids
    );
    assert!(
        !empty.subscribe_ids.contains(&3),
        "spot stays out of the observe set"
    );

    let bbo = |id, bid, ask, sz| {
        (
            id,
            BboUpdate {
                market_id: id,
                symbol: String::new(),
                nonce: 1,
                exchange_timestamp_ms: 1,
                bid_price: bid,
                bid_size: sz,
                ask_price: ask,
                ask_size: sz,
            },
        )
    };
    let ranked = resolve_live_universe(
        "auto",
        &[1],
        &catalog,
        &[
            bbo(1, 10.0, 10.10, 4.0),
            bbo(2, 20.0, 20.01, 0.1),
            bbo(3, 15.0, 15.01, 9.0),
            bbo(9, 80.0, 80.02, 5.0),
        ],
        &params,
    );
    assert!(
        !ranked.quoting_ids.contains(&1),
        "wide-spread id=1 must be filtered once books exist: {:?}",
        ranked.quoting_ids
    );
    assert!(
        !ranked.quoting_ids.contains(&2),
        "thin book must stay out: {:?}",
        ranked.quoting_ids
    );
    assert!(
        ranked.quoting_ids.contains(&9),
        "tight perp must win after BBO rank: {:?}",
        ranked.quoting_ids
    );
    assert_ne!(
        empty.quoting_ids, ranked.quoting_ids,
        "feeding books must change the quoting set"
    );
}

#[test]
fn mm_universe_prefers_wider_healthy_spreads() {
    let params = UniverseSelectParams {
        max_spread_bps: 25.0,
        min_spread_bps: 8.0,
        min_size: 1.0,
        prefer_wider_spreads: true,
        refresh_interval: Duration::from_secs(15),
        actions_per_refresh: 3,
    };
    let catalog = vec![
        fixture_market(1, "BTC", "perp", 100.0, 100.02, 5.0, 5.0, BookHealth::Live),
        fixture_market(5, "LIT", "perp", 2.0, 2.004, 4.0, 4.0, BookHealth::Live),
        fixture_market(36, "WIDE", "perp", 10.0, 10.10, 3.0, 3.0, BookHealth::Live),
    ];
    let selected = select_quoting_universe(&catalog, &params);
    let names: Vec<&str> = selected.iter().map(|m| m.symbol.as_str()).collect();
    assert!(
        !names.contains(&"BTC"),
        "sub-8bps BTC should stay out of maker universe: {names:?}"
    );
    assert!(
        !names.contains(&"WIDE"),
        "50bps+ junk stays out: {names:?}"
    );
    assert!(
        names.contains(&"LIT"),
        "mid-spread healthy perp should be selected: {names:?}"
    );
}
