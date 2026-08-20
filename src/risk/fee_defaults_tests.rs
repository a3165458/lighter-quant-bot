use config::{Config, File, FileFormat};

use super::profitability::{
    ProfitabilityGuard, DEFAULT_ADVERSE_SELECTION_BPS, DEFAULT_ENTRY_FEE_BPS, DEFAULT_EXIT_FEE_BPS,
};

fn settings(yaml: &str) -> Config {
    Config::builder()
        .add_source(File::from_str(yaml, FileFormat::Yaml))
        .build()
        .expect("test config")
}

fn load_yaml_file(path: &str) -> Config {
    Config::builder()
        .add_source(config::File::with_name(path))
        .build()
        .unwrap_or_else(|e| panic!("failed to load {path}: {e}"))
}

#[test]
fn missing_profitability_keys_use_nonzero_friction_floors() {
    let guard = ProfitabilityGuard::from_config(&settings(
        r#"
profitability:
  enabled: true
"#,
    ))
    .expect("defaults must be valid");

    assert!(
        guard.entry_fee_bps() > 0.0 && guard.exit_fee_bps() > 0.0,
        "fee defaults must not silently assume advertised 0"
    );
    assert!((guard.entry_fee_bps() - DEFAULT_ENTRY_FEE_BPS).abs() < 1e-12);
    assert!((guard.exit_fee_bps() - DEFAULT_EXIT_FEE_BPS).abs() < 1e-12);
    assert!(guard.entry_slippage_bps() > 0.0);
    assert!(guard.exit_slippage_bps() > 0.0);
    assert!(guard.adverse_selection_bps() > 0.0);
    assert!((guard.adverse_selection_bps() - DEFAULT_ADVERSE_SELECTION_BPS).abs() < 1e-12);
    assert!(guard.total_cost_bps() > 0.0);
}

#[test]
fn shipped_live_configs_do_not_assume_zero_fees_or_commission() {
    for path in ["config/settings.yaml", "config/settings.robinhood.yaml"] {
        let settings = load_yaml_file(path);
        let entry = settings
            .get_float("profitability.entry_fee_bps")
            .unwrap_or_else(|e| panic!("{path} entry_fee_bps: {e}"));
        let exit = settings
            .get_float("profitability.exit_fee_bps")
            .unwrap_or_else(|e| panic!("{path} exit_fee_bps: {e}"));
        let slip_in = settings
            .get_float("profitability.entry_slippage_bps")
            .unwrap_or_else(|e| panic!("{path} entry_slippage_bps: {e}"));
        let slip_out = settings
            .get_float("profitability.exit_slippage_bps")
            .unwrap_or_else(|e| panic!("{path} exit_slippage_bps: {e}"));
        let adverse = settings
            .get_float("profitability.adverse_selection_bps")
            .unwrap_or_else(|e| panic!("{path} adverse_selection_bps: {e}"));
        let commission = settings
            .get_float("backtest.commission_percent")
            .unwrap_or_else(|e| panic!("{path} commission_percent: {e}"));

        assert!(entry > 0.0, "{path} entry_fee_bps must be a positive floor");
        assert!(exit > 0.0, "{path} exit_fee_bps must be a positive floor");
        assert!(
            slip_in > 0.0 && slip_out > 0.0,
            "{path} slippage floors must be positive"
        );
        assert!(
            adverse > 0.0,
            "{path} adverse_selection_bps must be a positive floor"
        );
        assert!(
            commission > 0.0,
            "{path} backtest.commission_percent must be a positive floor, got {commission}"
        );
        assert!(
            commission >= 0.0001,
            "{path} commission must be at least 1 bp/side, got {commission}"
        );
    }
}

#[test]
fn shipped_robinhood_live_keeps_trend_following_only() {
    let settings = load_yaml_file("config/settings.robinhood.yaml");
    assert!(
        !settings
            .get_bool("trading.strategies.grid_trading.enabled")
            .unwrap_or(true),
        "RH live must not re-enable grid"
    );
    assert!(
        settings
            .get_bool("trading.strategies.trend_following.enabled")
            .unwrap_or(false),
        "RH live must keep trend_following enabled"
    );
    assert!(
        !settings
            .get_bool("trading.strategies.market_making.enabled")
            .unwrap_or(true),
        "RH live must not re-enable vol_obi MM"
    );
    assert!(
        settings.get_bool("profitability.enabled").unwrap_or(false),
        "RH live must require the profitability gate"
    );
    let notional = settings
        .get_float("trading.strategies.trend_following.notional")
        .expect("RH trend notional");
    assert!(
        notional > 0.0 && notional <= 100.0,
        "RH trend notional should stay tightened, got {notional}"
    );
}
