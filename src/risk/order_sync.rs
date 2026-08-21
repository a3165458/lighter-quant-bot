/// Consecutive empty exchange snapshots required before local working-order
/// state is reset to the exchange (0).
///
/// A single empty `accountActiveOrders` page can be truncated or flaky.
/// Two confirmed empties — or one empty plus an immediate confirm fetch —
/// is treated as "the book really has no working orders".
pub const EMPTY_OPEN_ORDER_CONFIRMATIONS: u32 = 2;

/// What the live loop should do after comparing exchange vs local counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenOrderReconcileAction {
    /// Exchange count is authoritative (non-empty, or both sides already 0).
    TrustExchange,
    /// First empty snapshot while local still thinks orders exist — hold local
    /// for this cycle only and wait for confirmation.
    HoldPendingConfirm,
    /// Confirmed empty book: drop the ghost local working-order count.
    ReconcileToExchange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOrderReconcile {
    pub trusted_count: u32,
    pub consecutive_empty: u32,
    pub action: OpenOrderReconcileAction,
}

/// Decide whether a live open-order snapshot should update local state.
///
/// `consecutive_empty` is the streak *before* this snapshot. Pass `>= 1`
/// when this cycle already confirmed a second empty fetch.
pub fn reconcile_open_order_count(
    exchange_count: u32,
    local_count: u32,
    consecutive_empty: u32,
) -> OpenOrderReconcile {
    if exchange_count > 0 {
        return OpenOrderReconcile {
            trusted_count: exchange_count,
            consecutive_empty: 0,
            action: OpenOrderReconcileAction::TrustExchange,
        };
    }

    if local_count == 0 {
        return OpenOrderReconcile {
            trusted_count: 0,
            consecutive_empty: 0,
            action: OpenOrderReconcileAction::TrustExchange,
        };
    }

    let next_empty = consecutive_empty.saturating_add(1);
    if next_empty >= EMPTY_OPEN_ORDER_CONFIRMATIONS {
        OpenOrderReconcile {
            trusted_count: 0,
            consecutive_empty: 0,
            action: OpenOrderReconcileAction::ReconcileToExchange,
        }
    } else {
        OpenOrderReconcile {
            trusted_count: local_count,
            consecutive_empty: next_empty,
            action: OpenOrderReconcileAction::HoldPendingConfirm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusts_nonempty_exchange_and_clears_empty_streak() {
        let decided = reconcile_open_order_count(3, 1, 1);
        assert_eq!(decided.trusted_count, 3);
        assert_eq!(decided.consecutive_empty, 0);
        assert_eq!(decided.action, OpenOrderReconcileAction::TrustExchange);
    }

    #[test]
    fn trusts_matching_empty_book() {
        let decided = reconcile_open_order_count(0, 0, 4);
        assert_eq!(decided.trusted_count, 0);
        assert_eq!(decided.consecutive_empty, 0);
        assert_eq!(decided.action, OpenOrderReconcileAction::TrustExchange);
    }

    #[test]
    fn first_empty_while_local_has_orders_holds_for_confirm() {
        let decided = reconcile_open_order_count(0, 1, 0);
        assert_eq!(decided.trusted_count, 1);
        assert_eq!(decided.consecutive_empty, 1);
        assert_eq!(decided.action, OpenOrderReconcileAction::HoldPendingConfirm);
    }

    #[test]
    fn confirmed_empty_reconciles_local_ghost_to_exchange() {
        let first = reconcile_open_order_count(0, 1, 0);
        let confirmed = reconcile_open_order_count(0, first.trusted_count, first.consecutive_empty);
        assert_eq!(confirmed.trusted_count, 0);
        assert_eq!(confirmed.consecutive_empty, 0);
        assert_eq!(
            confirmed.action,
            OpenOrderReconcileAction::ReconcileToExchange
        );
    }

    #[test]
    fn same_cycle_confirm_fetch_reconciles_immediately() {
        // Live loop treats a second empty fetch this cycle as consecutive_empty >= 1.
        let decided = reconcile_open_order_count(0, 2, 1);
        assert_eq!(decided.trusted_count, 0);
        assert_eq!(
            decided.action,
            OpenOrderReconcileAction::ReconcileToExchange
        );
    }

    #[test]
    fn never_keeps_ignoring_empty_exchange_forever() {
        let mut local = 1u32;
        let mut streak = 0u32;
        let mut saw_reconcile = false;
        for _ in 0..4 {
            let decided = reconcile_open_order_count(0, local, streak);
            local = decided.trusted_count;
            streak = decided.consecutive_empty;
            if decided.action == OpenOrderReconcileAction::ReconcileToExchange {
                saw_reconcile = true;
                break;
            }
        }
        assert!(saw_reconcile, "ghost working orders must be dropped");
        assert_eq!(local, 0);
        assert_eq!(streak, 0);
    }
}
