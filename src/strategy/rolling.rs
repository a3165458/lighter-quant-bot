//! Welford rolling mean/std/z-score. Port of djienne/LIGHTER_Market_Making_Rust
//! `src/strategy/rolling.rs` (itself a bit-exact port of `_vol_obi_fast.pyx`).

#[derive(Debug, Clone)]
pub struct RollingStats {
    buffer: Box<[f64]>,
    capacity: usize,
    write_pos: usize,
    count: usize,
    sum: f64,
    m2: f64,
    cached_mean: f64,
    cached_std: f64,
}

impl RollingStats {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be > 0");
        Self {
            buffer: vec![0.0; capacity].into_boxed_slice(),
            capacity,
            write_pos: 0,
            count: 0,
            sum: 0.0,
            m2: 0.0,
            cached_mean: 0.0,
            cached_std: 0.0,
        }
    }

    #[inline]
    pub fn push(&mut self, value: f64) {
        let idx = self.write_pos;

        if self.count >= self.capacity {
            let old = self.buffer[idx];
            let n = self.count;
            let old_mean = self.cached_mean;
            let new_mean = if n > 1 {
                old_mean + (old_mean - old) / (n as f64 - 1.0)
            } else {
                0.0
            };
            self.m2 -= (old - old_mean) * (old - new_mean);
            if self.m2 < 0.0 {
                self.m2 = 0.0;
            }
            self.sum -= old;
            self.count -= 1;
            self.cached_mean = new_mean;
        }

        self.buffer[idx] = value;
        self.sum += value;
        self.count += 1;
        self.write_pos += 1;
        if self.write_pos >= self.capacity {
            self.write_pos = 0;
        }
        let n = self.count;
        let old_mean = self.cached_mean;
        let new_mean = self.sum / n as f64;
        self.m2 += (value - old_mean) * (value - new_mean);
        if self.m2 < 0.0 {
            self.m2 = 0.0;
        }
        self.cached_mean = new_mean;
        self.cached_std = if n >= 2 {
            (self.m2 / n as f64).sqrt()
        } else {
            0.0
        };
    }

    #[inline]
    pub fn mean(&self) -> f64 {
        self.cached_mean
    }

    #[inline]
    pub fn std(&self) -> f64 {
        self.cached_std
    }

    #[inline]
    pub fn zscore(&self, value: f64) -> f64 {
        if self.cached_std < 1e-10 {
            return 0.0;
        }
        (value - self.cached_mean) / self.cached_std
    }

    #[inline]
    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.count
    }

    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.write_pos = 0;
        self.count = 0;
        self.sum = 0.0;
        self.m2 = 0.0;
        self.cached_mean = 0.0;
        self.cached_std = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_std_population() {
        let mut s = RollingStats::new(5);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            s.push(v);
        }
        assert!((s.mean() - 3.0).abs() < 1e-9);
        assert!((s.std() - 2.0_f64.sqrt()).abs() < 1e-9);
        assert!(s.zscore(3.0).abs() < 1e-9);
        assert!((s.zscore(5.0) - 2.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn eviction_matches_window() {
        let mut s = RollingStats::new(3);
        for v in [1.0, 2.0, 3.0] {
            s.push(v);
        }
        assert!((s.mean() - 2.0).abs() < 1e-9);
        s.push(4.0);
        assert!((s.mean() - 3.0).abs() < 1e-9);
        s.push(5.0);
        assert!((s.mean() - 4.0).abs() < 1e-9);
        assert!((s.std() - (2.0_f64 / 3.0).sqrt()).abs() < 1e-9);
    }

    #[test]
    fn warmup_zero_std() {
        let mut s = RollingStats::new(10);
        s.push(42.0);
        assert_eq!(s.std(), 0.0);
        assert_eq!(s.zscore(99.0), 0.0);
    }
}
