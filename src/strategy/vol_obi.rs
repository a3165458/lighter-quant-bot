//! Volatility + order-book-imbalance quote engine.
//!
//! Port of djienne/LIGHTER_Market_Making_Rust `src/strategy/vol_obi.rs`.
//! Samples mid changes and book imbalance on a `step_ns` clock; `quote()`
//! places a fair price at `mid + c1 * alpha` and a vol-scaled half-spread
//! floored by `min_half_spread_bps`. External alpha (Binance OBI) is injected
//! via `set_alpha_override`.

use super::rolling::RollingStats;

fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    x.max(lo).min(hi)
}

/// Sum size on bids with price >= `lower`.
pub fn sum_bid_from(levels: &[(f64, f64)], lower: f64) -> f64 {
    levels
        .iter()
        .filter(|(px, _)| *px >= lower)
        .map(|(_, qty)| *qty)
        .sum()
}

/// Sum size on asks with price <= `upper`.
pub fn sum_ask_to(levels: &[(f64, f64)], upper: f64) -> f64 {
    levels
        .iter()
        .filter(|(px, _)| *px <= upper)
        .map(|(_, qty)| *qty)
        .sum()
}

/// Infer a tick from a two-sided book, falling back to a mid-based decade.
pub fn tick_size_from_levels(bids: &[(f64, f64)], asks: &[(f64, f64)], mid: f64) -> f64 {
    let step = |levels: &[(f64, f64)]| {
        if levels.len() >= 2 {
            let t = (levels[0].0 - levels[1].0).abs();
            if t.is_finite() && t > 0.0 {
                return Some(t);
            }
        }
        None
    };
    if let Some(t) = step(bids).or_else(|| step(asks)) {
        return t;
    }
    if !mid.is_finite() || mid <= 0.0 {
        return 0.01;
    }
    let exp = mid.log10().floor() as i32 - 4;
    10f64.powi(exp).max(1e-8)
}

#[derive(Debug, Clone)]
pub struct VolObiConfig {
    pub window_steps: usize,
    pub step_ns: i64,
    pub vol_to_half_spread: f64,
    pub min_half_spread_bps: f64,
    pub c1_ticks: f64,
    pub c1: f64,
    pub skew: f64,
    pub looking_depth: f64,
    pub min_warmup_samples: i64,
}

impl Default for VolObiConfig {
    fn default() -> Self {
        Self {
            window_steps: 6000,
            step_ns: 100_000_000,
            // RH Standard venue fees are 0. Keep a thin floor for adverse
            // selection, not a fake commission pad. Width ≈ 4 bps.
            vol_to_half_spread: 6.0,
            min_half_spread_bps: 2.0,
            c1_ticks: 40.0,
            c1: 0.0,
            skew: 0.1,
            looking_depth: 0.025,
            min_warmup_samples: 100,
        }
    }
}

#[derive(Debug)]
pub struct VolObiCalculator {
    mid_stats: RollingStats,
    imb_stats: RollingStats,
    prev_mid: f64,
    has_prev_mid: bool,
    volatility: f64,
    alpha: f64,
    local_alpha: f64,
    alpha_override: f64,
    has_alpha_override: bool,
    warmed_up: bool,
    total_samples: i64,
    tick_size: f64,
    step_ns: i64,
    last_sample_ns: i64,
    vol_scale: f64,
    vol_to_half_spread: f64,
    min_half_spread_bps: f64,
    c1: f64,
    skew: f64,
    looking_depth: f64,
    min_warmup_samples: i64,
    max_position_dollar: f64,
}

impl VolObiCalculator {
    pub fn new(cfg: &VolObiConfig, tick_size: f64, max_position_dollar: f64) -> Self {
        let tick = if tick_size > 0.0 { tick_size } else { 0.01 };
        let c1 = if cfg.c1 > 0.0 {
            cfg.c1
        } else {
            cfg.c1_ticks * tick
        };
        Self {
            mid_stats: RollingStats::new(cfg.window_steps.max(2)),
            imb_stats: RollingStats::new(cfg.window_steps.max(2)),
            prev_mid: 0.0,
            has_prev_mid: false,
            volatility: 0.0,
            alpha: 0.0,
            local_alpha: 0.0,
            alpha_override: 0.0,
            has_alpha_override: false,
            warmed_up: false,
            total_samples: 0,
            tick_size: tick,
            step_ns: cfg.step_ns.max(1),
            last_sample_ns: 0,
            vol_scale: (1_000_000_000.0 / cfg.step_ns.max(1) as f64).sqrt(),
            vol_to_half_spread: cfg.vol_to_half_spread,
            min_half_spread_bps: cfg.min_half_spread_bps,
            c1,
            skew: cfg.skew,
            looking_depth: cfg.looking_depth,
            min_warmup_samples: cfg.min_warmup_samples.max(1),
            max_position_dollar,
        }
    }

    pub fn on_book_update(
        &mut self,
        now_ns: i64,
        mid_price: f64,
        bids: &[(f64, f64)],
        asks: &[(f64, f64)],
    ) {
        if !mid_price.is_finite() || mid_price <= 0.0 {
            return;
        }
        let sample_due = self.last_sample_ns == 0 || now_ns - self.last_sample_ns >= self.step_ns;

        if sample_due {
            if self.has_prev_mid {
                self.mid_stats.push(mid_price - self.prev_mid);
                self.total_samples += 1;
            }
            self.prev_mid = mid_price;
            self.has_prev_mid = true;
            self.last_sample_ns = now_ns;
        }

        let lower = mid_price * (1.0 - self.looking_depth);
        let upper = mid_price * (1.0 + self.looking_depth);
        let imbalance = sum_bid_from(bids, lower) - sum_ask_to(asks, upper);
        if sample_due {
            self.imb_stats.push(imbalance);
        }

        if self.total_samples >= self.min_warmup_samples {
            self.warmed_up = true;
            if sample_due {
                self.volatility = self.mid_stats.std() * self.vol_scale;
            }
            self.local_alpha = self.imb_stats.zscore(imbalance);
            self.alpha = if self.has_alpha_override {
                self.alpha_override
            } else {
                self.local_alpha
            };
        }
    }

    pub fn quote(&self, mid_price: f64, position_size: f64) -> Option<(f64, f64)> {
        if !self.warmed_up || !mid_price.is_finite() || mid_price <= 0.0 {
            return None;
        }
        let tick = self.tick_size;
        let half_spread_price = self.volatility * self.vol_to_half_spread;
        let half_spread_tick = half_spread_price / tick;
        let fair_price = mid_price + self.c1 * self.alpha;

        let norm_pos = if self.max_position_dollar > 0.0 {
            clamp(
                (position_size * mid_price) / self.max_position_dollar,
                -1.0,
                1.0,
            )
        } else {
            0.0
        };

        let mut bid_depth_tick = half_spread_tick * (1.0 + self.skew * norm_pos);
        let mut ask_depth_tick = half_spread_tick * (1.0 - self.skew * norm_pos);
        bid_depth_tick = bid_depth_tick.max(0.0);
        ask_depth_tick = ask_depth_tick.max(0.0);

        let mut raw_bid = fair_price - bid_depth_tick * tick;
        let mut raw_ask = fair_price + ask_depth_tick * tick;

        if self.min_half_spread_bps > 0.0 {
            let min_bid = mid_price * (1.0 - self.min_half_spread_bps / 10_000.0);
            if raw_bid > min_bid {
                raw_bid = min_bid;
            }
            let min_ask = mid_price * (1.0 + self.min_half_spread_bps / 10_000.0);
            if raw_ask < min_ask {
                raw_ask = min_ask;
            }
        }

        let bid_price = (raw_bid / tick).floor() * tick;
        let ask_price = (raw_ask / tick).ceil() * tick;
        if bid_price >= ask_price || bid_price <= 0.0 {
            return None;
        }
        Some((bid_price, ask_price))
    }

    pub fn set_alpha_override(&mut self, alpha: Option<f64>) {
        match alpha {
            None => {
                self.has_alpha_override = false;
                if self.warmed_up {
                    self.alpha = self.local_alpha;
                }
            }
            Some(a) => {
                self.has_alpha_override = true;
                self.alpha_override = a;
                if self.warmed_up {
                    self.alpha = a;
                }
            }
        }
    }

    pub fn set_max_position_dollar(&mut self, value: f64) {
        self.max_position_dollar = if value > 0.0 { value } else { 0.0 };
    }

    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.mid_stats.clear();
        self.imb_stats.clear();
        self.prev_mid = 0.0;
        self.has_prev_mid = false;
        self.volatility = 0.0;
        self.alpha = 0.0;
        self.local_alpha = 0.0;
        self.has_alpha_override = false;
        self.warmed_up = false;
        self.total_samples = 0;
        self.last_sample_ns = 0;
    }

    pub fn warmed_up(&self) -> bool {
        self.warmed_up
    }

    #[allow(dead_code)]
    pub fn volatility(&self) -> f64 {
        self.volatility
    }

    pub fn alpha(&self) -> f64 {
        self.alpha
    }

    pub fn total_samples(&self) -> i64 {
        self.total_samples
    }
}

/// Passive reducing quote when the engine is not ready but inventory is open.
pub fn fallback_reduce_only(mid: f64, position: f64, tick: f64, fallback_bps: f64) -> Option<(f64, f64)> {
    if position.abs() < 1e-12 || mid <= 0.0 {
        return None;
    }
    let min_depth = if tick > 0.0 { tick } else { (mid * 1e-6).max(1e-9) };
    let depth = (mid * fallback_bps.max(1.0) / 10_000.0).max(min_depth);
    if position > 0.0 {
        let mut ask = ((mid + depth) / tick.max(1e-12)).ceil() * tick.max(1e-12);
        if ask <= mid {
            ask = mid + min_depth;
        }
        Some((f64::NAN, ask))
    } else {
        let mut bid = ((mid - depth) / tick.max(1e-12)).floor() * tick.max(1e-12);
        if bid >= mid || bid <= 0.0 {
            bid = mid - min_depth;
        }
        Some((bid, f64::NAN))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_warmed_returns_none() {
        let c = VolObiCalculator::new(&VolObiConfig::default(), 0.1, 1000.0);
        assert_eq!(c.quote(100.0, 0.0), None);
    }

    #[test]
    fn warms_up_and_quotes_outside_mid() {
        let cfg = VolObiConfig {
            min_warmup_samples: 5,
            window_steps: 100,
            min_half_spread_bps: 2.0,
            vol_to_half_spread: 1.0,
            c1_ticks: 0.0,
            skew: 0.0,
            ..Default::default()
        };
        let mut c = VolObiCalculator::new(&cfg, 0.1, 1_000_000.0);
        let bids = [(99.0, 1.0), (98.5, 1.0)];
        let asks = [(101.0, 1.0), (101.5, 1.0)];
        for i in 0..12 {
            let mid = 100.5 + i as f64 * 0.1;
            c.on_book_update((i as i64 + 1) * 100_000_000, mid, &bids, &asks);
        }
        assert!(c.warmed_up());
        let (bid, ask) = c.quote(101.0, 0.0).unwrap();
        assert!(bid < 101.0 && 101.0 < ask);
        let half_bps = (101.0 - bid) / 101.0 * 10_000.0;
        assert!(half_bps + 1e-6 >= 2.0);
    }

    #[test]
    fn samples_on_step_clock() {
        let cfg = VolObiConfig {
            min_warmup_samples: 1,
            window_steps: 100,
            ..Default::default()
        };
        let mut c = VolObiCalculator::new(&cfg, 0.1, 1000.0);
        let bids = [(99.0, 1.0)];
        let asks = [(101.0, 1.0)];
        for i in 0..20i64 {
            c.on_book_update((i + 1) * 50_000_000, 100.0 + i as f64 * 0.1, &bids, &asks);
        }
        assert_eq!(c.total_samples(), 9);
    }

    #[test]
    fn alpha_override_roundtrip() {
        let cfg = VolObiConfig {
            min_warmup_samples: 2,
            window_steps: 50,
            ..Default::default()
        };
        let mut c = VolObiCalculator::new(&cfg, 0.1, 1000.0);
        let bids = [(99.0, 1.0)];
        let asks = [(101.0, 1.0)];
        for i in 0..5 {
            c.on_book_update((i as i64 + 1) * 100_000_000, 100.0, &bids, &asks);
        }
        c.set_alpha_override(Some(2.5));
        assert!((c.alpha() - 2.5).abs() < 1e-12);
        c.set_alpha_override(None);
        assert!(c.alpha().is_finite());
    }

    #[test]
    fn imbalance_sums_depth_window() {
        let bids = [(100.0, 2.0), (99.0, 3.0), (97.0, 9.0)];
        let asks = [(101.0, 1.0), (102.0, 4.0), (104.0, 8.0)];
        assert!((sum_bid_from(&bids, 98.0) - 5.0).abs() < 1e-12);
        assert!((sum_ask_to(&asks, 102.0) - 5.0).abs() < 1e-12);
    }
}
