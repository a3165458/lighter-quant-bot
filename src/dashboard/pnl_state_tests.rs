use super::pnl_accounting::fill_stats_from_history;
use super::server::DashboardState;
use serde_json::json;
use std::collections::HashMap;

fn inflated_state() -> super::server::PersistentPnlData {
    let mut daily = HashMap::new();
    daily.insert("2026-08-17".to_string(), 1040.0);
    daily.insert("2026-08-18".to_string(), 1125.0);
    super::server::PersistentPnlData {
        total_realized_pnl: 324.0,
        initial_equity: 1295.0,
        peak_equity: 1296.0,
        equity_history: vec![(1, 1295.0), (2, 1085.0)],
        pnl_history: vec![(1, 0.0), (2, -210.0)],
        trade_history: vec![
            json!({"action":"Open","price":3500.0,"quantity":200.0,"symbol":"ETH"}),
            json!({"action":"Add","price":500.0,"quantity":80.0,"symbol":"QQQ"}),
            json!({"action":"Full Close","price":2500.0,"quantity":0.2,"symbol":"ETH"}),
        ],
        daily_pnl_map: daily,
        total_volume: 3_834_579.0,
        total_closed_trades: 705,
        total_order_notional: 0.0,
    }
}

#[test]
fn restore_rebases_inflated_volume_closes_and_daily_map() {
    let mut ds = DashboardState {
        equity: 1085.0,
        initial_equity: 1295.0,
        unrealized_pnl: 0.0,
        ..DashboardState::default()
    };
    let changed = ds.restore_pnl(&inflated_state());
    assert!(changed);
    assert!((ds.total_volume - 500.0).abs() < 1e-9);
    assert_eq!(ds.total_closed_trades, 1);
    assert_eq!(ds.trade_history.len(), 1);
    assert!((ds.total_realized_pnl - (1085.0 - 1295.0)).abs() < 1e-9);
    assert!(!ds.daily_pnl_map.contains_key("2026-08-17"));
    assert!(!ds.daily_pnl_map.contains_key("2026-08-18"));
}

#[test]
fn push_trade_ignores_placement_and_counts_fills_only() {
    let mut ds = DashboardState::default();
    ds.push_trade(json!({
        "action": "Open",
        "price": 100.0,
        "quantity": 10.0,
        "symbol": "ETH",
    }));
    assert_eq!(ds.total_volume, 0.0);
    assert_eq!(ds.total_closed_trades, 0);
    assert!(ds.trade_history.is_empty());

    ds.record_order_placement(100.0, 10.0);
    assert!((ds.total_order_notional - 1000.0).abs() < 1e-9);
    assert_eq!(ds.total_volume, 0.0);

    ds.push_trade(json!({
        "action": "Open",
        "fill": true,
        "price": 100.0,
        "quantity": 2.0,
        "symbol": "ETH",
    }));
    ds.push_trade(json!({
        "action": "Full Close",
        "fill": true,
        "price": 100.0,
        "quantity": 2.0,
        "symbol": "ETH",
    }));
    assert!((ds.total_volume - 400.0).abs() < 1e-9);
    assert_eq!(ds.total_closed_trades, 1);
    assert_eq!(ds.trade_history.len(), 2);
    assert_eq!(ds.total_trades, 2);

    let (vol, closes) = fill_stats_from_history(&ds.trade_history);
    assert!((vol - 400.0).abs() < 1e-9);
    assert_eq!(closes, 1);
}

#[test]
fn push_trade_does_not_count_stop_substring_as_close() {
    let mut ds = DashboardState::default();
    ds.push_trade(json!({
        "action": "stop",
        "price": 50.0,
        "quantity": 1.0,
    }));
    assert_eq!(ds.total_closed_trades, 0);
    assert_eq!(ds.total_volume, 0.0);
}
