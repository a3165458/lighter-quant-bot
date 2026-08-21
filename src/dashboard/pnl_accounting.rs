//! Live PnL identity and snapshot-glitch guards.
//!
//! Exchange equity already includes unrealized. The only internally consistent
//! "since inception" dollar PnL is `equity - initial_equity`. Incremental
//! `Δequity - Δunrealized` is only safe when a real close happened and equity
//! moved with the vanished unrealized.
//!
//! Dashboard volume / close counts must be exchange-confirmed fills. Successful
//! order *placement* (including unfilled maker requotes) is not a fill.

use serde_json::Value;
use std::collections::HashMap;

/// Mark-to-market PnL since the stored initial equity.
pub fn inception_pnl(equity: f64, initial_equity: f64) -> f64 {
    if !equity.is_finite() || !initial_equity.is_finite() || initial_equity <= 0.0 {
        return 0.0;
    }
    equity - initial_equity
}

/// Implied lifetime realized from the accounting identity
/// `equity = initial + realized + unrealized` (no deposits/withdrawals).
pub fn implied_realized(equity: f64, initial_equity: f64, unrealized: f64) -> f64 {
    inception_pnl(equity, initial_equity) - unrealized
}

/// True when stored realized has drifted from the equity identity by more than
/// a few dollars (false Full Close from an incomplete account snapshot).
pub fn realized_needs_rebase(stored_realized: f64, implied: f64) -> bool {
    stored_realized.is_finite() && implied.is_finite() && (stored_realized - implied).abs() > 10.0
}

/// Book incremental realized from a detected position reduction, or `None` when
/// the snapshot looks incomplete (positions vanished, equity barely moved).
pub fn realized_from_position_reductions(
    equity_change: f64,
    unrealized_change: f64,
    closed_notional: f64,
    all_positions_vanished: bool,
) -> Option<f64> {
    if !equity_change.is_finite() || !unrealized_change.is_finite() {
        return None;
    }
    let realized = equity_change - unrealized_change;
    if !realized.is_finite() {
        return None;
    }

    // Positions dropped off the book but equity did not give back the uPnL.
    // That is the 2026-08-14 +$329 false Full Close (ETH+BTC vanished, equity flat).
    if all_positions_vanished
        && unrealized_change < -1.0
        && equity_change.abs() < (-unrealized_change * 0.25).max(1.0)
    {
        return None;
    }
    if closed_notional > 0.0 && realized.abs() > closed_notional * 1.5 && realized.abs() > 10.0 {
        return None;
    }
    Some(realized)
}

/// First equity sample on the UTC day of `now_ts`, else `fallback`.
pub fn start_of_day_equity(history: &[(i64, f64)], now_ts: i64, fallback: f64) -> f64 {
    if now_ts <= 0 {
        return fallback;
    }
    let day = now_ts.div_euclid(86_400);
    history
        .iter()
        .find(|(ts, eq)| *ts > 0 && ts.div_euclid(86_400) == day && eq.is_finite())
        .map(|(_, eq)| *eq)
        .filter(|eq| *eq > 0.0)
        .unwrap_or(fallback)
}

pub fn daily_mark_to_market(equity: f64, start_of_day: f64) -> f64 {
    if !equity.is_finite() || !start_of_day.is_finite() || start_of_day <= 0.0 {
        return 0.0;
    }
    equity - start_of_day
}

/// Real position-close actions recorded from an account-size reduction.
pub fn action_is_close_fill(action: &str) -> bool {
    matches!(
        action.trim(),
        "Full Close" | "Partial Close" | "Emergency Close" | "Liquidation"
    )
}

fn trade_action(rec: &Value) -> &str {
    rec.get("action")
        .or_else(|| rec.get("close_type"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A fill is either explicitly tagged or a legacy close recorded from the
/// position book. Untagged Open/Add rows are order placements, not fills.
pub fn trade_is_fill(rec: &Value) -> bool {
    match rec.get("fill").and_then(Value::as_bool) {
        Some(true) => true,
        Some(false) => false,
        None => action_is_close_fill(trade_action(rec)),
    }
}

pub fn fill_notional(rec: &Value) -> f64 {
    let price = rec.get("price").and_then(Value::as_f64).unwrap_or(0.0);
    let qty = rec.get("quantity").and_then(Value::as_f64).unwrap_or(0.0);
    if price.is_finite() && qty.is_finite() {
        (price * qty).abs()
    } else {
        0.0
    }
}

/// Fill notional and real close count from a history buffer.
pub fn fill_stats_from_history(history: &[Value]) -> (f64, u64) {
    let mut volume = 0.0_f64;
    let mut closes = 0_u64;
    for rec in history {
        if !trade_is_fill(rec) {
            continue;
        }
        volume += fill_notional(rec);
        if action_is_close_fill(trade_action(rec)) {
            closes += 1;
        }
    }
    (volume, closes)
}

/// Open vs Add from a confirmed position-size increase (same book snapshot).
pub fn position_increase_fill_action(
    prev_size: f64,
    new_size: f64,
    same_side: bool,
    min_change: f64,
) -> Option<&'static str> {
    let min_change = if min_change.is_finite() && min_change > 0.0 {
        min_change
    } else {
        1e-12
    };
    if !new_size.is_finite() || new_size.abs() < min_change {
        return None;
    }
    if !same_side {
        return Some("Open");
    }
    let prev = if prev_size.is_finite() {
        prev_size.abs()
    } else {
        0.0
    };
    if prev < min_change {
        return Some("Open");
    }
    if new_size.abs() - prev >= min_change {
        return Some("Add");
    }
    None
}

pub fn min_equity_hint(
    equity_history: &[(i64, f64)],
    peak_equity: f64,
    initial_equity: f64,
    constructor_equity: f64,
) -> f64 {
    let hist_min = equity_history
        .iter()
        .map(|(_, equity)| *equity)
        .filter(|equity| equity.is_finite() && *equity > 0.0)
        .fold(f64::INFINITY, f64::min);
    if hist_min.is_finite() {
        return hist_min;
    }
    [constructor_equity, initial_equity, peak_equity]
        .into_iter()
        .filter(|equity| equity.is_finite() && *equity > 0.0)
        .fold(f64::INFINITY, f64::min)
}

/// One-day realized cannot exceed the cash the book has ever moved by much.
pub fn daily_pnl_sanity_cap(
    peak_equity: f64,
    min_equity: f64,
    initial_equity: f64,
    implied_or_stored: f64,
) -> f64 {
    let mut span = 0.0_f64;
    for (left, right) in [
        (peak_equity, min_equity),
        (peak_equity, initial_equity),
        (min_equity, initial_equity),
    ] {
        if left.is_finite() && right.is_finite() && left > 0.0 && right > 0.0 {
            span = span.max((left - right).abs());
        }
    }
    if implied_or_stored.is_finite() {
        span = span.max(implied_or_stored.abs());
    }
    span.max(1.0) * 1.5
}

pub fn sanitize_daily_pnl_map(
    map: &HashMap<String, f64>,
    cap: f64,
    implied_realized: Option<f64>,
) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = map
        .iter()
        .filter(|(_, pnl)| pnl.is_finite() && pnl.abs() <= cap + 1e-9)
        .map(|(day, pnl)| (day.clone(), *pnl))
        .collect();
    if let Some(implied) = implied_realized.filter(|value| value.is_finite()) {
        let sum: f64 = out.values().sum();
        if (sum - implied).abs() > cap.max(50.0) {
            out.clear();
        }
    }
    out
}

/// Inputs for load-time correction of a persisted dashboard PnL file.
pub struct PersistedPnlInputs<'a> {
    pub trade_history: Vec<Value>,
    pub total_volume: f64,
    pub total_closed_trades: u64,
    pub total_realized_pnl: f64,
    pub daily_pnl_map: HashMap<String, f64>,
    pub peak_equity: f64,
    pub equity_history: &'a [(i64, f64)],
    pub initial_equity: f64,
    pub constructor_equity: f64,
    pub unrealized: f64,
}

#[derive(Clone, Debug)]
pub struct CorrectedPersistedPnl {
    pub trade_history: Vec<Value>,
    pub total_volume: f64,
    pub total_closed_trades: u64,
    pub total_realized_pnl: f64,
    pub daily_pnl_map: HashMap<String, f64>,
    pub changed: bool,
}

/// Drop placement rows, recompute fill volume/closes, rebase realized to the
/// cash-equity identity, and strip daily-map days the equity range cannot
/// support. Inflated lifetime counters are discarded when the ring still
/// contains non-fill placements (the old "every successful quote is a fill"
/// bug). Clean fill-only files keep lifetime counters so aged-out fills
/// are not forgotten.
pub fn correct_persisted_pnl(input: PersistedPnlInputs<'_>) -> CorrectedPersistedPnl {
    let raw_history = input.trade_history;
    let non_fill_count = raw_history.iter().filter(|rec| !trade_is_fill(rec)).count();
    let trade_history: Vec<Value> = raw_history.into_iter().filter(trade_is_fill).collect();
    let (hist_volume, hist_closes) = fill_stats_from_history(&trade_history);
    // Placement rows still in the ring, or lifetime counters that cannot
    // exist given cash equity + the retained fill buffer.
    let implausible_lifetime = input.peak_equity > 0.0
        && input.total_volume > input.peak_equity * 500.0
        && input.total_volume > hist_volume.max(1.0) * 20.0
        && input.total_closed_trades > hist_closes.saturating_mul(5).max(10);
    let polluted = non_fill_count > 0 || implausible_lifetime;

    let total_volume = if polluted {
        hist_volume
    } else if input.total_volume.is_finite() && input.total_volume > 0.0 {
        input.total_volume.max(hist_volume)
    } else {
        hist_volume
    };
    let total_closed_trades = if polluted {
        hist_closes
    } else if input.total_closed_trades > 0 {
        input.total_closed_trades.max(hist_closes)
    } else {
        hist_closes
    };

    let can_rebase_realized = input.constructor_equity > 0.0 && input.initial_equity > 0.0;
    let implied = if can_rebase_realized {
        implied_realized(
            input.constructor_equity,
            input.initial_equity,
            input.unrealized,
        )
    } else {
        input.total_realized_pnl
    };
    let total_realized_pnl =
        if can_rebase_realized && realized_needs_rebase(input.total_realized_pnl, implied) {
            implied
        } else {
            input.total_realized_pnl
        };

    let min_equity = min_equity_hint(
        input.equity_history,
        input.peak_equity,
        input.initial_equity,
        input.constructor_equity,
    );
    let cap = daily_pnl_sanity_cap(
        input.peak_equity,
        min_equity,
        input.initial_equity,
        if can_rebase_realized {
            implied
        } else {
            total_realized_pnl
        },
    );
    let daily_implied = can_rebase_realized.then_some(implied);
    let daily_pnl_map = sanitize_daily_pnl_map(&input.daily_pnl_map, cap, daily_implied);

    let changed = trade_history.len() != non_fill_count.saturating_add(trade_history.len())
        || (total_volume - input.total_volume).abs() > 1e-6
        || total_closed_trades != input.total_closed_trades
        || (total_realized_pnl - input.total_realized_pnl).abs() > 1e-6
        || daily_pnl_map != input.daily_pnl_map;

    CorrectedPersistedPnl {
        trade_history,
        total_volume,
        total_closed_trades,
        total_realized_pnl,
        daily_pnl_map,
        changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inception_matches_equity_minus_initial() {
        assert!((inception_pnl(1290.18, 1295.566) - (1290.18 - 1295.566)).abs() < 1e-12);
        assert_eq!(inception_pnl(100.0, 0.0), 0.0);
    }

    #[test]
    fn vanished_book_with_flat_equity_is_not_realized_profit() {
        // prev uPnL +304, positions empty, equity unchanged → old code booked +304.
        let booked = realized_from_position_reductions(0.05, -304.15, 800.0, true);
        assert!(
            booked.is_none(),
            "incomplete snapshot must not mint realized PnL, got {booked:?}"
        );
    }

    #[test]
    fn real_close_that_locks_in_unrealized_is_booked() {
        // Winning long closed at mark: cash +10, uPnL 10 → 0.
        let booked = realized_from_position_reductions(0.0, -10.0, 200.0, false);
        assert_eq!(booked, Some(10.0));
    }

    #[test]
    fn rebase_detects_the_aug14_drift() {
        let implied = implied_realized(1290.18, 1295.566, 0.0);
        assert!(realized_needs_rebase(324.40, implied));
        assert!(!realized_needs_rebase(implied, implied));
    }

    #[test]
    fn large_identity_gap_is_still_detected_but_is_a_glitch() {
        // 2026-08-15: stored -107 vs implied -207 while live equity stayed ~$1288.
        let implied = implied_realized(1288.65, 1295.566, 200.0);
        assert!(realized_needs_rebase(-106.93, implied));
        assert!((implied + 206.916).abs() < 0.01);
    }

    #[test]
    fn start_of_day_uses_first_sample_on_that_utc_day() {
        let day = 20_680_i64; // arbitrary
        let ts0 = day * 86_400 + 100;
        let ts1 = day * 86_400 + 500;
        let hist = [(ts0, 1000.0), (ts1, 1005.0)];
        assert_eq!(start_of_day_equity(&hist, ts1 + 10, 999.0), 1000.0);
        assert_eq!(daily_mark_to_market(994.0, 1000.0), -6.0);
    }

    fn placement_open(price: f64, qty: f64) -> Value {
        serde_json::json!({
            "action": "Open",
            "price": price,
            "quantity": qty,
            "side": "Buy",
            "symbol": "ETH",
        })
    }

    fn fill_open(price: f64, qty: f64) -> Value {
        serde_json::json!({
            "action": "Open",
            "fill": true,
            "price": price,
            "quantity": qty,
            "side": "Buy",
            "symbol": "ETH",
        })
    }

    fn full_close(price: f64, qty: f64) -> Value {
        serde_json::json!({
            "action": "Full Close",
            "price": price,
            "quantity": qty,
            "side": "Sell",
            "symbol": "ETH",
        })
    }

    #[test]
    fn placement_open_is_not_fill_volume() {
        let placement = placement_open(3500.0, 1.0);
        assert!(!trade_is_fill(&placement));
        let (volume, closes) = fill_stats_from_history(&[placement, fill_open(3500.0, 0.1)]);
        assert!((volume - 350.0).abs() < 1e-9);
        assert_eq!(closes, 0);
    }

    #[test]
    fn close_count_is_exact_actions_not_substrings() {
        assert!(action_is_close_fill("Full Close"));
        assert!(action_is_close_fill("Partial Close"));
        assert!(action_is_close_fill("Emergency Close"));
        assert!(action_is_close_fill("Liquidation"));
        assert!(!action_is_close_fill("unclosed"));
        assert!(!action_is_close_fill("stop"));
        assert!(!action_is_close_fill("Take Profit Stop"));
        assert!(!action_is_close_fill("Open"));
        let (volume, closes) = fill_stats_from_history(&[
            serde_json::json!({"action": "stop", "price": 100.0, "quantity": 1.0}),
            full_close(100.0, 0.5),
            serde_json::json!({"action": "Partial Close", "price": 50.0, "quantity": 1.0}),
        ]);
        assert_eq!(closes, 2);
        assert!((volume - 100.0).abs() < 1e-9);
    }

    #[test]
    fn position_increase_classifies_open_and_add() {
        assert_eq!(
            position_increase_fill_action(0.0, 0.01, true, 0.001),
            Some("Open")
        );
        assert_eq!(
            position_increase_fill_action(0.01, 0.02, true, 0.001),
            Some("Add")
        );
        assert_eq!(position_increase_fill_action(0.01, 0.01, true, 0.001), None);
        assert_eq!(
            position_increase_fill_action(0.01, 0.02, false, 0.001),
            Some("Open")
        );
    }

    #[test]
    fn load_time_rebase_drops_placement_volume_and_phantom_daily_days() {
        let mut daily = HashMap::new();
        daily.insert("2026-08-17".to_string(), 1040.0);
        daily.insert("2026-08-18".to_string(), 1125.0);
        daily.insert("2026-08-20".to_string(), -12.0);
        let history = vec![
            placement_open(3500.0, 200.0),
            placement_open(500.0, 100.0),
            fill_open(2500.0, 1.0),
            full_close(2500.0, 0.2),
        ];
        let equity_history = vec![(1, 1295.0), (2, 1085.0)];
        let corrected = correct_persisted_pnl(PersistedPnlInputs {
            trade_history: history,
            total_volume: 3_834_579.0,
            total_closed_trades: 705,
            total_realized_pnl: 324.0,
            daily_pnl_map: daily,
            peak_equity: 1296.0,
            equity_history: &equity_history,
            initial_equity: 1295.0,
            constructor_equity: 1085.0,
            unrealized: 0.0,
        });
        assert!(corrected.changed);
        assert_eq!(corrected.trade_history.len(), 2);
        assert!((corrected.total_volume - 3000.0).abs() < 1e-9);
        assert_eq!(corrected.total_closed_trades, 1);
        assert!((corrected.total_realized_pnl - (1085.0 - 1295.0)).abs() < 1e-9);
        assert!(
            !corrected.daily_pnl_map.contains_key("2026-08-17"),
            "a +$1k day is impossible when peak-min cash never moved that far"
        );
        assert!(!corrected.daily_pnl_map.contains_key("2026-08-18"));
    }

    #[test]
    fn clean_fill_only_state_keeps_lifetime_counters() {
        let history = vec![fill_open(100.0, 1.0), full_close(100.0, 1.0)];
        let corrected = correct_persisted_pnl(PersistedPnlInputs {
            trade_history: history,
            total_volume: 50_000.0,
            total_closed_trades: 40,
            total_realized_pnl: -5.0,
            daily_pnl_map: HashMap::new(),
            peak_equity: 1300.0,
            equity_history: &[(1, 1295.0)],
            initial_equity: 1295.0,
            constructor_equity: 1290.0,
            unrealized: 0.0,
        });
        assert!((corrected.total_volume - 50_000.0).abs() < 1e-9);
        assert_eq!(corrected.total_closed_trades, 40);
        assert_eq!(corrected.trade_history.len(), 2);
    }

    #[test]
    fn implausible_lifetime_counters_are_reset_even_if_placements_aged_out() {
        let corrected = correct_persisted_pnl(PersistedPnlInputs {
            trade_history: vec![full_close(2500.0, 0.2)],
            total_volume: 3_834_579.0,
            total_closed_trades: 705,
            total_realized_pnl: -215.0,
            daily_pnl_map: HashMap::new(),
            peak_equity: 1296.0,
            equity_history: &[(1, 1085.0)],
            initial_equity: 1295.0,
            constructor_equity: 1085.0,
            unrealized: 0.0,
        });
        assert!((corrected.total_volume - 500.0).abs() < 1e-9);
        assert_eq!(corrected.total_closed_trades, 1);
    }
}
