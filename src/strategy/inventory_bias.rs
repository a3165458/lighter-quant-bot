//! Inventory exit bias + quality-spread widening.
//! Port of djienne quote-ladder transforms (`apply_inventory_exit_bias`,
//! `apply_quality_spread_multiplier`) for a single two-sided quote.

#[derive(Debug, Clone, Copy)]
pub struct InventoryExitBias {
    pub enabled: bool,
    pub min_ratio: f64,
    pub exit_tighten_per_ratio: f64,
    pub add_widen_per_ratio: f64,
    pub max_exit_tighten: f64,
    pub max_add_widen: f64,
    pub adverse_boost_per_bps: f64,
}

impl Default for InventoryExitBias {
    fn default() -> Self {
        Self {
            enabled: true,
            min_ratio: 0.05,
            exit_tighten_per_ratio: 0.45,
            add_widen_per_ratio: 0.75,
            max_exit_tighten: 0.35,
            max_add_widen: 0.65,
            adverse_boost_per_bps: 0.03,
        }
    }
}

fn floor_tick(p: f64, tick: f64) -> f64 {
    if tick > 0.0 {
        (p / tick).floor() * tick
    } else {
        p
    }
}

fn ceil_tick(p: f64, tick: f64) -> f64 {
    if tick > 0.0 {
        (p / tick).ceil() * tick
    } else {
        p
    }
}

/// Widen both sides away from mid when `multiplier` > 1 (adverse markouts).
pub fn apply_quality_spread_multiplier(
    bid: f64,
    ask: f64,
    mid: f64,
    multiplier: f64,
    tick: f64,
) -> (f64, f64) {
    if multiplier <= 1.0001 || mid <= 0.0 {
        return (bid, ask);
    }
    let bid_out = if bid.is_finite() && bid > 0.0 {
        floor_tick(mid - (mid - bid).max(0.0) * multiplier, tick)
    } else {
        bid
    };
    let ask_out = if ask.is_finite() && ask > 0.0 {
        ceil_tick(mid + (ask - mid).max(0.0) * multiplier, tick)
    } else {
        ask
    };
    (bid_out, ask_out)
}

/// Tighten the reducing side and widen the adding side as inventory grows.
pub fn apply_inventory_exit_bias(
    bid: f64,
    ask: f64,
    mid: f64,
    position: f64,
    max_pos_usd: f64,
    adverse_bps: f64,
    adverse_threshold_bps: f64,
    cfg: &InventoryExitBias,
    tick: f64,
) -> (f64, f64) {
    if !cfg.enabled || mid <= 0.0 || max_pos_usd <= 0.0 || position.abs() < 1e-12 {
        return (bid, ask);
    }
    let ratio = (position.abs() * mid) / max_pos_usd;
    if ratio < cfg.min_ratio {
        return (bid, ask);
    }
    let adverse_excess = (adverse_bps - adverse_threshold_bps).max(0.0);
    let boost = 1.0 + (adverse_excess * cfg.adverse_boost_per_bps.max(0.0)).min(0.5);
    let exit_tighten =
        (cfg.exit_tighten_per_ratio.max(0.0) * ratio * boost).min(cfg.max_exit_tighten.max(0.0));
    let add_widen =
        (cfg.add_widen_per_ratio.max(0.0) * ratio * boost).min(cfg.max_add_widen.max(0.0));
    if exit_tighten <= 0.0 && add_widen <= 0.0 {
        return (bid, ask);
    }

    let min_depth = if tick > 0.0 {
        tick
    } else {
        (mid * 1e-6).max(1e-9)
    };
    let mut bid_out = bid;
    let mut ask_out = ask;
    if bid.is_finite() && bid > 0.0 {
        let mut depth = (mid - bid).max(min_depth);
        if position < 0.0 {
            depth *= (1.0 - exit_tighten).max(0.05);
        } else {
            depth *= 1.0 + add_widen;
        }
        let mut nb = floor_tick(mid - depth, tick);
        if nb >= mid {
            nb = floor_tick(mid - min_depth, tick);
        }
        bid_out = nb;
    }
    if ask.is_finite() && ask > 0.0 {
        let mut depth = (ask - mid).max(min_depth);
        if position > 0.0 {
            depth *= (1.0 - exit_tighten).max(0.05);
        } else {
            depth *= 1.0 + add_widen;
        }
        let mut na = ceil_tick(mid + depth, tick);
        if na <= mid {
            na = ceil_tick(mid + min_depth, tick);
        }
        ask_out = na;
    }
    (bid_out, ask_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quality_multiplier_widens() {
        let (b, a) = apply_quality_spread_multiplier(99.0, 101.0, 100.0, 1.5, 0.1);
        assert!((b - 98.5).abs() < 1e-9);
        assert!((a - 101.5).abs() < 1e-9);
    }

    #[test]
    fn long_inventory_tightens_ask_widens_bid() {
        let cfg = InventoryExitBias::default();
        let (b, a) =
            apply_inventory_exit_bias(99.0, 101.0, 100.0, 1.0, 100.0, 0.0, 2.0, &cfg, 0.1);
        assert!(a < 101.0, "exit ask should move toward mid");
        assert!(b < 99.0, "add bid should move away from mid");
    }
}
