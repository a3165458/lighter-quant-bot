pub mod engine;
pub mod margin;
pub mod metrics;
pub mod replay;
pub mod results;

/// 2 bps per side (0.02%). Floor used when yaml omits `commission_percent`
/// or sets it to 0 without a measured fill-level fee of exactly 0.
/// This is execution friction, not advertised maker/taker marketing.
pub const DEFAULT_COMMISSION_RATE: f64 = 0.0002;

/// Read `backtest.commission_percent`, refusing a silent zero default.
pub fn commission_rate_from_config(settings: &config::Config) -> f64 {
    settings
        .get_float("backtest.commission_percent")
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(DEFAULT_COMMISSION_RATE)
}

#[cfg(test)]
mod commission_default_tests {
    use super::*;
    use config::{Config, File, FileFormat};

    #[test]
    fn missing_or_zero_commission_uses_positive_floor() {
        let missing = Config::builder().build().expect("empty config");
        assert!(commission_rate_from_config(&missing) > 0.0);
        assert_eq!(
            commission_rate_from_config(&missing),
            DEFAULT_COMMISSION_RATE
        );

        let zero = Config::builder()
            .add_source(File::from_str(
                "backtest:\n  commission_percent: 0.0\n",
                FileFormat::Yaml,
            ))
            .build()
            .expect("zero commission yaml");
        assert!(commission_rate_from_config(&zero) > 0.0);
        assert_eq!(commission_rate_from_config(&zero), DEFAULT_COMMISSION_RATE);
    }

    #[test]
    fn explicit_positive_commission_is_honored() {
        let settings = Config::builder()
            .add_source(File::from_str(
                "backtest:\n  commission_percent: 0.00015\n",
                FileFormat::Yaml,
            ))
            .build()
            .expect("positive commission yaml");
        assert!((commission_rate_from_config(&settings) - 0.00015).abs() < 1e-12);
    }
}
