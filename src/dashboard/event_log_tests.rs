use super::event_log::{
    reconcile_events_from_trades, DashboardEvent, DashboardEventKind, EventLog, EventSnapshot,
    EventTracker, EVENT_HISTORY_LIMIT,
};
use serde_json::json;

fn trade(timestamp: &str, price: f64) -> serde_json::Value {
    json!({
        "timestamp": timestamp,
        "symbol": "BTC",
        "side": "Buy",
        "price": price,
        "quantity": 0.001,
        "fill": true,
    })
}

#[test]
fn tracker_ignores_baseline_and_emits_only_real_state_changes() {
    let baseline = EventSnapshot {
        open_orders: 5,
        trading_paused: false,
        risk_status: Some(json!({
            "drawdown_pct": -0.20,
            "daily_loss_pct": 0.01,
            "is_healthy": true,
        })),
        trade_history: vec![trade("2026-08-03T05:00:00Z", 62_700.0)],
    };
    let mut tracker = EventTracker::default();
    assert!(tracker.observe(&baseline, 1_785_730_000_000).is_empty());

    let changed = EventSnapshot {
        open_orders: 3,
        trading_paused: true,
        risk_status: Some(json!({
            "drawdown_pct": -0.27,
            "daily_loss_pct": 0.01,
            "is_healthy": true,
        })),
        trade_history: vec![
            trade("2026-08-03T05:00:00Z", 62_700.0),
            trade("2026-08-03T05:00:03Z", 62_769.38),
        ],
    };
    let events = tracker.observe(&changed, 1_785_730_003_000);

    assert_eq!(events.len(), 4);
    assert_eq!(events[0].kind, DashboardEventKind::Order);
    assert_eq!(events[0].detail, "5 → 3");
    assert_eq!(events[1].kind, DashboardEventKind::State);
    assert_eq!(events[2].kind, DashboardEventKind::Risk);
    assert_eq!(events[3].kind, DashboardEventKind::Fill);
    assert!(events[3].detail.contains("Buy BTC @ 62769.38"));
    assert!(tracker.observe(&changed, 1_785_730_006_000).is_empty());
}

#[test]
fn event_log_round_trips_atomically_and_keeps_only_the_newest_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dashboard_events.json");
    let mut log = EventLog::default();

    for index in 0..EVENT_HISTORY_LIMIT + 7 {
        log.extend([DashboardEvent {
            timestamp_ms: index as i64,
            kind: DashboardEventKind::Risk,
            detail: format!("event-{index}"),
        }]);
    }

    assert_eq!(log.events().len(), EVENT_HISTORY_LIMIT);
    assert_eq!(log.events().first().unwrap().detail, "event-7");
    log.save_to(&path).unwrap();
    assert!(!path.with_extension("json.tmp").exists());

    let restored = EventLog::load_from(&path).unwrap();
    assert_eq!(restored.events(), log.events());
}

#[test]
fn old_or_missing_event_files_load_as_an_empty_log() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.json");
    assert!(EventLog::load_or_default(&missing).events().is_empty());

    let legacy = dir.path().join("legacy.json");
    std::fs::write(&legacy, "{}").unwrap();
    assert!(EventLog::load_or_default(&legacy).events().is_empty());
}

#[test]
fn submitted_orders_are_not_mislabeled_as_fills() {
    let mut tracker = EventTracker::default();
    tracker.observe(&EventSnapshot::default(), 1_785_730_000_000);

    let submitted = EventSnapshot {
        trade_history: vec![json!({
            "timestamp": "2026-08-03T05:00:03Z",
            "symbol": "BTC",
            "side": "Sell",
            "price": 62_834.5,
            "quantity": 0.00048,
            "action": "Open",
        })],
        ..EventSnapshot::default()
    };
    let events = tracker.observe(&submitted, 1_785_730_003_000);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, DashboardEventKind::Order);
    assert!(events[0].detail.starts_with("Open Sell BTC @ 62834.50"));
}

#[test]
fn confirmed_open_fill_is_a_fill_event() {
    let mut tracker = EventTracker::default();
    tracker.observe(&EventSnapshot::default(), 1_785_730_000_000);

    let filled = EventSnapshot {
        trade_history: vec![json!({
            "timestamp": "2026-08-03T05:00:03Z",
            "symbol": "BTC",
            "side": "Buy",
            "price": 62_834.5,
            "quantity": 0.00048,
            "action": "Open",
            "fill": true,
        })],
        ..EventSnapshot::default()
    };
    let events = tracker.observe(&filled, 1_785_730_003_000);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, DashboardEventKind::Fill);
}

#[test]
fn restart_reconciliation_recovers_only_trade_events_newer_than_the_saved_log() {
    let saved_at = chrono::DateTime::parse_from_rfc3339("2026-08-03T05:00:01Z")
        .unwrap()
        .timestamp_millis();
    let mut log = EventLog::from_events(vec![DashboardEvent {
        timestamp_ms: saved_at,
        kind: DashboardEventKind::Risk,
        detail: "saved risk".to_string(),
    }]);
    let trades = vec![
        trade("2026-08-03T05:00:00Z", 62_700.0),
        trade("2026-08-03T05:00:03Z", 62_769.38),
    ];

    assert!(reconcile_events_from_trades(&mut log, &trades));
    assert_eq!(log.events().len(), 2);
    assert_eq!(log.events()[0].detail, "saved risk");
    assert!(log.events()[1].detail.contains("62769.38"));
    assert!(!reconcile_events_from_trades(&mut log, &trades));
}
