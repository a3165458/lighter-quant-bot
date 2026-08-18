use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const AUDIT_FILE: &str = "quant_agent_audit.json";
const MAX_AUDIT_RECORDS: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BacktestEvidence {
    pub data_file: String,
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
    #[serde(default)]
    pub capital: f64,
    pub total_return_pct: f64,
    pub sharpe_ratio: f64,
    pub max_drawdown_pct: f64,
    pub total_trades: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProposalInput {
    pub strategy: String,
    pub params: HashMap<String, serde_json::Value>,
    pub evidence: BacktestEvidence,
    #[serde(default)]
    pub rationale: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PolicySnapshot {
    pub equity: f64,
    pub trading_paused: bool,
    pub emergency_triggered: bool,
    pub max_drawdown_pct: f64,
    pub max_notional_pct: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PolicyDecision {
    pub allowed: bool,
    pub checks: Vec<String>,
    pub violations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentProposal {
    pub id: String,
    pub created_at: String,
    pub status: String,
    pub input: ProposalInput,
    pub policy: PolicySnapshot,
    pub decision: PolicyDecision,
    pub approval_phrase: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AgentLedger {
    pub proposals: Vec<AgentProposal>,
}

fn numeric(params: &HashMap<String, serde_json::Value>, key: &str) -> Option<f64> {
    params.get(key).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
    })
}

pub fn evaluate_proposal(input: &ProposalInput, policy: &PolicySnapshot) -> PolicyDecision {
    let mut checks = vec![
        format!("evidence dataset: {}", input.evidence.data_file),
        format!("max drawdown <= {:.2}%", policy.max_drawdown_pct),
        format!("max notional <= {:.2}% equity", policy.max_notional_pct),
    ];
    let mut violations = Vec::new();

    if policy.trading_paused {
        violations.push("trading is paused".to_string());
    }
    if policy.emergency_triggered {
        violations.push("risk emergency is active".to_string());
    }
    if policy.equity <= 0.0 || !policy.equity.is_finite() {
        violations.push("account equity is unavailable".to_string());
    }
    let e = &input.evidence;
    if e.data_file.trim().is_empty() {
        violations.push("backtest dataset is missing".to_string());
    }
    if !e.total_return_pct.is_finite() || e.total_return_pct <= 0.0 {
        violations.push("verified backtest return must be positive".to_string());
    }
    if !e.sharpe_ratio.is_finite() || e.sharpe_ratio <= 0.0 {
        violations.push("verified Sharpe ratio must be positive".to_string());
    }
    if !e.max_drawdown_pct.is_finite() || e.max_drawdown_pct.abs() > policy.max_drawdown_pct {
        violations.push(format!(
            "verified drawdown exceeds {:.2}%",
            policy.max_drawdown_pct
        ));
    }
    if e.total_trades < 3 {
        violations.push("at least 3 verified trades are required".to_string());
    }

    let notional_cap = policy.equity * policy.max_notional_pct / 100.0;
    match input.strategy.as_str() {
        "grid" | "grid_trading" => {
            let count = numeric(&input.params, "grid_count");
            let investment = numeric(&input.params, "investment")
                .or_else(|| numeric(&input.params, "investment_per_grid"));
            let deviation = numeric(&input.params, "deviation")
                .or_else(|| numeric(&input.params, "price_deviation"));
            if !matches!(count, Some(v) if (4.0..=40.0).contains(&v)) {
                violations.push("grid_count must be between 4 and 40".to_string());
            }
            if !matches!(investment, Some(v) if v > 0.0 && v <= notional_cap) {
                violations.push(format!("grid investment exceeds ${notional_cap:.2} cap"));
            }
            if !matches!(deviation, Some(v) if (0.001..=0.05).contains(&v)) {
                violations.push("grid deviation must be between 0.001 and 0.05".to_string());
            }
        }
        "trend" | "trend_following" => {
            let fast = numeric(&input.params, "fast_ma");
            let slow = numeric(&input.params, "slow_ma");
            let notional = numeric(&input.params, "notional");
            if !matches!((fast, slow), (Some(f), Some(s)) if f >= 2.0 && f < s && s <= 500.0) {
                violations.push(
                    "trend moving averages must satisfy 2 <= fast_ma < slow_ma <= 500".to_string(),
                );
            }
            if !matches!(notional, Some(v) if v > 0.0 && v <= notional_cap) {
                violations.push(format!("trend notional exceeds ${notional_cap:.2} cap"));
            }
        }
        "mm" | "market_making" => {
            let bid = numeric(&input.params, "bid_spread");
            let ask = numeric(&input.params, "ask_spread");
            let notional = numeric(&input.params, "order_notional")
                .or_else(|| numeric(&input.params, "order_amount"));
            if !matches!(bid, Some(v) if (0.0001..=0.05).contains(&v)) {
                violations.push("mm bid_spread must be between 0.0001 and 0.05".to_string());
            }
            if !matches!(ask, Some(v) if (0.0001..=0.05).contains(&v)) {
                violations.push("mm ask_spread must be between 0.0001 and 0.05".to_string());
            }
            if !matches!(notional, Some(v) if v > 0.0 && v <= notional_cap) {
                violations.push(format!("mm order_notional exceeds ${notional_cap:.2} cap"));
            }
        }
        _ => violations.push("strategy is not in the live allowlist".to_string()),
    }

    checks.push("model has proposal authority only".to_string());
    PolicyDecision {
        allowed: violations.is_empty(),
        checks,
        violations,
    }
}

impl AgentLedger {
    pub fn load(network: &str) -> Self {
        let Ok(path) = super::runtime_paths::data_file(network, AUDIT_FILE) else {
            return Self::default();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, network: &str) -> std::io::Result<()> {
        let path =
            super::runtime_paths::data_file(network, AUDIT_FILE).map_err(std::io::Error::other)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn record(&mut self, proposal: AgentProposal) {
        self.proposals.push(proposal);
        if self.proposals.len() > MAX_AUDIT_RECORDS {
            self.proposals
                .drain(..self.proposals.len() - MAX_AUDIT_RECORDS);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_input() -> ProposalInput {
        ProposalInput {
            strategy: "trend".into(),
            params: HashMap::from([
                ("fast_ma".into(), serde_json::json!(14)),
                ("slow_ma".into(), serde_json::json!(50)),
                ("notional".into(), serde_json::json!(20)),
            ]),
            evidence: BacktestEvidence {
                data_file: "BTC.csv".into(),
                start: "2026-01-01".into(),
                end: "2026-01-31".into(),
                capital: 100.0,
                total_return_pct: 2.0,
                sharpe_ratio: 1.2,
                max_drawdown_pct: 3.0,
                total_trades: 8,
            },
            rationale: "verified candidate".into(),
        }
    }

    fn policy() -> PolicySnapshot {
        PolicySnapshot {
            equity: 100.0,
            trading_paused: false,
            emergency_triggered: false,
            max_drawdown_pct: 10.0,
            max_notional_pct: 25.0,
        }
    }

    #[test]
    fn allows_verified_bounded_proposal() {
        assert!(evaluate_proposal(&safe_input(), &policy()).allowed);
    }

    #[test]
    fn rejects_model_requested_notional_above_hard_cap() {
        let mut input = safe_input();
        input
            .params
            .insert("notional".into(), serde_json::json!(26));
        let decision = evaluate_proposal(&input, &policy());
        assert!(!decision.allowed);
        assert!(decision.violations.iter().any(|v| v.contains("cap")));
    }

    #[test]
    fn rejects_when_risk_emergency_is_active() {
        let mut p = policy();
        p.emergency_triggered = true;
        assert!(!evaluate_proposal(&safe_input(), &p).allowed);
    }
}
