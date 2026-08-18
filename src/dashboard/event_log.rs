use anyhow::{Context, Result};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::task::JoinHandle;
use tokio::time::{Duration, MissedTickBehavior};
use tracing::{info, warn};

pub const EVENT_HISTORY_FILE: &str = "dashboard_events.json";
pub const EVENT_HISTORY_LIMIT: usize = 200;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DashboardEventKind {
    Risk,
    Order,
    Fill,
    State,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DashboardEvent {
    pub timestamp_ms: i64,
    pub kind: DashboardEventKind,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventLog {
    #[serde(default)]
    events: Vec<DashboardEvent>,
}

impl EventLog {
    pub fn from_events(events: Vec<DashboardEvent>) -> Self {
        let mut log = Self::default();
        log.extend(events);
        log
    }

    pub fn events(&self) -> &[DashboardEvent] {
        &self.events
    }

    pub fn extend(&mut self, events: impl IntoIterator<Item = DashboardEvent>) {
        self.events.extend(events);
        self.trim();
    }

    fn trim(&mut self) {
        self.events.sort_by_key(|event| event.timestamp_ms);
        if self.events.len() > EVENT_HISTORY_LIMIT {
            self.events.drain(..self.events.len() - EVENT_HISTORY_LIMIT);
        }
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)
            .with_context(|| format!("failed to read event history from {}", path.display()))?;
        let mut log: Self = serde_json::from_str(&data)
            .with_context(|| format!("failed to parse event history from {}", path.display()))?;
        log.trim();
        Ok(log)
    }

    pub fn load_or_default(path: &Path) -> Self {
        Self::load_from(path).unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create event history directory {}",
                    parent.display()
                )
            })?;
        }

        let json = serde_json::to_vec_pretty(self).context("failed to serialize event history")?;
        let temp_path = temporary_path(path);
        let mut file = File::create(&temp_path).with_context(|| {
            format!(
                "failed to create temporary event history {}",
                temp_path.display()
            )
        })?;
        file.write_all(&json).with_context(|| {
            format!(
                "failed to write temporary event history {}",
                temp_path.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "failed to sync temporary event history {}",
                temp_path.display()
            )
        })?;
        fs::rename(&temp_path, path).with_context(|| {
            format!(
                "failed to replace event history {} with {}",
                path.display(),
                temp_path.display()
            )
        })?;
        if let Some(parent) = path.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .with_context(|| {
                    format!(
                        "failed to sync event history directory {}",
                        parent.display()
                    )
                })?;
        }
        Ok(())
    }
}

fn temporary_path(path: &Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!("{value}.tmp"))
        .unwrap_or_else(|| "tmp".to_string());
    path.with_extension(extension)
}

#[derive(Clone, Debug, Default)]
pub struct EventSnapshot {
    pub open_orders: u32,
    pub trading_paused: bool,
    pub risk_status: Option<Value>,
    pub trade_history: Vec<Value>,
}

#[derive(Default)]
pub struct EventTracker {
    initialized: bool,
    previous_open_orders: u32,
    previous_trading_paused: bool,
    previous_risk_key: Option<String>,
    latest_trade_key: Option<String>,
}

impl EventTracker {
    pub fn observe(&mut self, snapshot: &EventSnapshot, now_ms: i64) -> Vec<DashboardEvent> {
        let risk_key = snapshot.risk_status.as_ref().map(risk_key);
        let latest_trade_key = snapshot.trade_history.last().map(trade_key);

        if !self.initialized {
            self.initialized = true;
            self.previous_open_orders = snapshot.open_orders;
            self.previous_trading_paused = snapshot.trading_paused;
            self.previous_risk_key = risk_key;
            self.latest_trade_key = latest_trade_key;
            return Vec::new();
        }

        let mut events = Vec::new();
        if snapshot.open_orders != self.previous_open_orders {
            events.push(DashboardEvent {
                timestamp_ms: now_ms,
                kind: DashboardEventKind::Order,
                detail: format!("{} → {}", self.previous_open_orders, snapshot.open_orders),
            });
        }
        if snapshot.trading_paused != self.previous_trading_paused {
            events.push(DashboardEvent {
                timestamp_ms: now_ms,
                kind: DashboardEventKind::State,
                detail: if snapshot.trading_paused {
                    "TRADING PAUSED".to_string()
                } else {
                    "LIVE TRADING".to_string()
                },
            });
        }
        if risk_key != self.previous_risk_key {
            if let Some(risk) = snapshot.risk_status.as_ref() {
                events.push(DashboardEvent {
                    timestamp_ms: now_ms,
                    kind: DashboardEventKind::Risk,
                    detail: risk_detail(risk),
                });
            }
        }

        events.extend(self.new_trade_events(&snapshot.trade_history, now_ms));

        self.previous_open_orders = snapshot.open_orders;
        self.previous_trading_paused = snapshot.trading_paused;
        self.previous_risk_key = risk_key;
        self.latest_trade_key = latest_trade_key;
        events
    }

    fn new_trade_events(&self, trades: &[Value], fallback_ms: i64) -> Vec<DashboardEvent> {
        if trades.is_empty() {
            return Vec::new();
        }

        let start = self
            .latest_trade_key
            .as_ref()
            .and_then(|previous| {
                trades
                    .iter()
                    .position(|trade| trade_key(trade) == *previous)
            })
            .map(|index| index + 1)
            .unwrap_or(0);

        trades[start..]
            .iter()
            .map(|trade| event_from_trade(trade, fallback_ms))
            .collect()
    }
}

pub fn seed_events_from_trades(trades: &[Value]) -> Vec<DashboardEvent> {
    trades
        .iter()
        .rev()
        .take(EVENT_HISTORY_LIMIT)
        .rev()
        .map(|trade| event_from_trade(trade, chrono::Utc::now().timestamp_millis()))
        .collect()
}

pub fn reconcile_events_from_trades(log: &mut EventLog, trades: &[Value]) -> bool {
    let newest_saved = log.events().last().map(|event| event.timestamp_ms);
    let missing = seed_events_from_trades(trades)
        .into_iter()
        .filter(|event| newest_saved.is_none_or(|timestamp| event.timestamp_ms > timestamp))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return false;
    }
    log.extend(missing);
    true
}

fn risk_key(risk: &Value) -> String {
    [
        "drawdown_pct",
        "daily_loss_pct",
        "is_healthy",
        "emergency_triggered",
    ]
    .iter()
    .map(|key| risk.get(key).cloned().unwrap_or(Value::Null).to_string())
    .collect::<Vec<_>>()
    .join("|")
}

fn risk_detail(risk: &Value) -> String {
    let drawdown = risk
        .get("drawdown_pct")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let daily_loss = risk
        .get("daily_loss_pct")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    format!("DD {drawdown:.2}%  DL {daily_loss:.2}%")
}

fn trade_key(trade: &Value) -> String {
    ["timestamp", "price", "side", "quantity", "action"]
        .iter()
        .map(|key| trade.get(key).cloned().unwrap_or(Value::Null).to_string())
        .collect::<Vec<_>>()
        .join("|")
}

fn event_from_trade(trade: &Value, fallback_ms: i64) -> DashboardEvent {
    let side = trade.get("side").and_then(Value::as_str).unwrap_or("—");
    let symbol = trade
        .get("symbol")
        .or_else(|| trade.get("market"))
        .and_then(Value::as_str)
        .unwrap_or("—");
    let price = trade
        .get("price")
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0.0);
    let decimals = if price.abs() < 100.0 { 4 } else { 2 };
    let timestamp_ms = trade
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis())
        .unwrap_or(fallback_ms);
    let action = trade.get("action").and_then(Value::as_str).unwrap_or("");
    let is_submitted_order = matches!(action, "Open" | "Add");
    let detail = if action.is_empty() {
        format!("{side} {symbol} @ {price:.decimals$}")
    } else {
        format!("{action} {side} {symbol} @ {price:.decimals$}")
    };

    DashboardEvent {
        timestamp_ms,
        kind: if is_submitted_order {
            DashboardEventKind::Order
        } else {
            DashboardEventKind::Fill
        },
        detail,
    }
}

pub async fn restore_event_history(state: &super::server::SharedDashboardState) {
    let (network, trades) = {
        let ds = state.read().await;
        (ds.network_name.clone(), ds.trade_history.clone())
    };
    let Ok(path) = super::runtime_paths::data_file(&network, EVENT_HISTORY_FILE) else {
        warn!("⚠️ Invalid network for dashboard event history: {network}");
        return;
    };
    let mut log = EventLog::load_or_default(&path);
    let reconciled = reconcile_events_from_trades(&mut log, &trades);

    {
        let mut dashboard = state.write().await;
        dashboard.event_history = log.events().to_vec();
    }

    if reconciled {
        if let Err(error) = log.save_to(&path) {
            warn!("⚠️ Failed to seed dashboard event history: {error}");
        }
    }
    info!(
        "📂 Restored {} dashboard events from {}",
        log.events().len(),
        path.display()
    );
}

pub fn spawn_event_monitor(state: super::server::SharedDashboardState) -> JoinHandle<()> {
    tokio::spawn(async move {
        let network = state.read().await.network_name.clone();
        let Ok(path) = super::runtime_paths::data_file(&network, EVENT_HISTORY_FILE) else {
            warn!("⚠️ Invalid network for dashboard event monitor: {network}");
            return;
        };
        let mut tracker = EventTracker::default();
        let baseline = snapshot_from_state(&state).await;
        tracker.observe(&baseline, chrono::Utc::now().timestamp_millis());

        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        interval.tick().await;

        loop {
            interval.tick().await;
            let snapshot = snapshot_from_state(&state).await;
            let events = tracker.observe(&snapshot, chrono::Utc::now().timestamp_millis());
            if events.is_empty() {
                continue;
            }

            let log = {
                let mut dashboard = state.write().await;
                let mut log = EventLog::from_events(dashboard.event_history.clone());
                log.extend(events);
                dashboard.event_history = log.events().to_vec();
                log
            };

            let save_path = path.clone();
            if let Err(error) = tokio::task::spawn_blocking(move || log.save_to(&save_path))
                .await
                .unwrap_or_else(|error| Err(anyhow::anyhow!(error)))
            {
                warn!("⚠️ Failed to persist dashboard event history: {error}");
            }
        }
    })
}

async fn snapshot_from_state(state: &super::server::SharedDashboardState) -> EventSnapshot {
    let dashboard = state.read().await;
    EventSnapshot {
        open_orders: dashboard.open_orders,
        trading_paused: dashboard.trading_paused,
        risk_status: dashboard.risk_status.clone(),
        trade_history: dashboard.trade_history.clone(),
    }
}
