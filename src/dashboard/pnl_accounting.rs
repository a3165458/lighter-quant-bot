//! Live PnL identity and snapshot-glitch guards.
//!
//! Exchange equity already includes unrealized. The only internally consistent
//! "since inception" dollar PnL is `equity - initial_equity`. Incremental
//! `Δequity - Δunrealized` is only safe when a real close happened and equity
//! moved with the vanished unrealized.

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
    stored_realized.is_finite()
        && implied.is_finite()
        && (stored_realized - implied).abs() > 10.0
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
}
