use anyhow::{bail, Result};
use config::Config;

const CONFIG_PREFIX: &str = "profitability";

/// Conservative friction floors used when a key is omitted.
/// Advertised 0 maker/taker is marketing, not a measured fill-level fee of 0.
pub const DEFAULT_ENTRY_FEE_BPS: f64 = 1.0;
pub const DEFAULT_EXIT_FEE_BPS: f64 = 1.0;
pub const DEFAULT_ENTRY_SLIPPAGE_BPS: f64 = 1.0;
pub const DEFAULT_EXIT_SLIPPAGE_BPS: f64 = 1.0;
pub const DEFAULT_FUNDING_BPS: f64 = 0.5;
pub const DEFAULT_ADVERSE_SELECTION_BPS: f64 = 2.0;
pub const DEFAULT_MIN_NET_EDGE_BPS: f64 = 1.0;

/// Strategy-provided economics for one proposed order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignalEconomics {
    pub expected_edge_bps: Option<f64>,
    pub risk_reducing: bool,
}

impl SignalEconomics {
    pub fn entry(expected_edge_bps: Option<f64>) -> Self {
        Self {
            expected_edge_bps,
            risk_reducing: false,
        }
    }

    pub fn exit() -> Self {
        Self {
            expected_edge_bps: None,
            risk_reducing: true,
        }
    }
}

/// Auditable result returned by [`ProfitabilityGuard::evaluate`].
#[derive(Debug, Clone, PartialEq)]
pub struct ProfitabilityDecision {
    pub allowed: bool,
    pub expected_edge_bps: Option<f64>,
    pub total_cost_bps: f64,
    pub net_edge_bps: Option<f64>,
    pub required_net_edge_bps: f64,
    pub reason: &'static str,
}

/// Conservative round-trip execution-cost model.
#[derive(Debug, Clone)]
pub struct ProfitabilityGuard {
    enabled: bool,
    entry_fee_bps: f64,
    exit_fee_bps: f64,
    entry_slippage_bps: f64,
    exit_slippage_bps: f64,
    funding_bps: f64,
    adverse_selection_bps: f64,
    min_net_edge_bps: f64,
}

impl ProfitabilityGuard {
    pub fn from_config(settings: &Config) -> Result<Self> {
        let guard = Self {
            enabled: get_bool(settings, "enabled", true),
            entry_fee_bps: get_float(settings, "entry_fee_bps", DEFAULT_ENTRY_FEE_BPS),
            exit_fee_bps: get_float(settings, "exit_fee_bps", DEFAULT_EXIT_FEE_BPS),
            entry_slippage_bps: get_float(
                settings,
                "entry_slippage_bps",
                DEFAULT_ENTRY_SLIPPAGE_BPS,
            ),
            exit_slippage_bps: get_float(settings, "exit_slippage_bps", DEFAULT_EXIT_SLIPPAGE_BPS),
            funding_bps: get_float(settings, "funding_bps", DEFAULT_FUNDING_BPS),
            adverse_selection_bps: get_float(
                settings,
                "adverse_selection_bps",
                DEFAULT_ADVERSE_SELECTION_BPS,
            ),
            min_net_edge_bps: get_float(settings, "min_net_edge_bps", DEFAULT_MIN_NET_EDGE_BPS),
        };
        guard.validate()?;
        Ok(guard)
    }

    pub fn evaluate(&self, economics: SignalEconomics) -> ProfitabilityDecision {
        let total_cost_bps = self.total_cost_bps();

        if economics.risk_reducing {
            return ProfitabilityDecision {
                allowed: true,
                expected_edge_bps: economics.expected_edge_bps,
                total_cost_bps,
                net_edge_bps: economics
                    .expected_edge_bps
                    .map(|edge| edge - total_cost_bps),
                required_net_edge_bps: self.min_net_edge_bps,
                reason: "risk_reducing_exit",
            };
        }

        if !self.enabled {
            return ProfitabilityDecision {
                allowed: true,
                expected_edge_bps: economics.expected_edge_bps,
                total_cost_bps,
                net_edge_bps: economics
                    .expected_edge_bps
                    .map(|edge| edge - total_cost_bps),
                required_net_edge_bps: self.min_net_edge_bps,
                reason: "guard_disabled",
            };
        }

        let Some(expected_edge_bps) = economics.expected_edge_bps else {
            return ProfitabilityDecision {
                allowed: false,
                expected_edge_bps: None,
                total_cost_bps,
                net_edge_bps: None,
                required_net_edge_bps: self.min_net_edge_bps,
                reason: "missing_expected_edge",
            };
        };

        if !expected_edge_bps.is_finite() || expected_edge_bps < 0.0 {
            return ProfitabilityDecision {
                allowed: false,
                expected_edge_bps: Some(expected_edge_bps),
                total_cost_bps,
                net_edge_bps: None,
                required_net_edge_bps: self.min_net_edge_bps,
                reason: "invalid_expected_edge",
            };
        }

        let net_edge_bps = expected_edge_bps - total_cost_bps;
        let allowed = net_edge_bps > self.min_net_edge_bps;
        ProfitabilityDecision {
            allowed,
            expected_edge_bps: Some(expected_edge_bps),
            total_cost_bps,
            net_edge_bps: Some(net_edge_bps),
            required_net_edge_bps: self.min_net_edge_bps,
            reason: if allowed {
                "positive_net_edge"
            } else {
                "insufficient_net_edge"
            },
        }
    }

    pub fn total_cost_bps(&self) -> f64 {
        self.entry_fee_bps
            + self.exit_fee_bps
            + self.entry_slippage_bps
            + self.exit_slippage_bps
            + self.funding_bps
            + self.adverse_selection_bps
    }

    pub fn entry_fee_bps(&self) -> f64 {
        self.entry_fee_bps
    }

    pub fn exit_fee_bps(&self) -> f64 {
        self.exit_fee_bps
    }

    pub fn entry_slippage_bps(&self) -> f64 {
        self.entry_slippage_bps
    }

    pub fn exit_slippage_bps(&self) -> f64 {
        self.exit_slippage_bps
    }

    pub fn adverse_selection_bps(&self) -> f64 {
        self.adverse_selection_bps
    }

    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("entry_fee_bps", self.entry_fee_bps),
            ("exit_fee_bps", self.exit_fee_bps),
            ("funding_bps", self.funding_bps),
        ] {
            if !value.is_finite() {
                bail!("{CONFIG_PREFIX}.{name} must be finite");
            }
        }

        for (name, value) in [
            ("entry_slippage_bps", self.entry_slippage_bps),
            ("exit_slippage_bps", self.exit_slippage_bps),
            ("adverse_selection_bps", self.adverse_selection_bps),
            ("min_net_edge_bps", self.min_net_edge_bps),
        ] {
            if !value.is_finite() || value < 0.0 {
                bail!("{CONFIG_PREFIX}.{name} must be finite and >= 0");
            }
        }

        if self.total_cost_bps() < 0.0 {
            bail!("{CONFIG_PREFIX} total cost must be >= 0 after rebates");
        }
        Ok(())
    }
}

fn get_float(settings: &Config, key: &str, default: f64) -> f64 {
    settings
        .get_float(&format!("{CONFIG_PREFIX}.{key}"))
        .unwrap_or(default)
}

fn get_bool(settings: &Config, key: &str, default: bool) -> bool {
    settings
        .get_bool(&format!("{CONFIG_PREFIX}.{key}"))
        .unwrap_or(default)
}
