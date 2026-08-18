use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use futures::{SinkExt, StreamExt};
use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::{timeout, Duration};
use tower_http::cors::CorsLayer;
use tracing::{info, warn};

const PNL_STATE_FILE: &str = "pnl_state.json";

/// Max fills kept in the live trade history buffer (also used by /api/pnl).
/// Order-placement and close-event paths must share this constant — previously
/// one path kept 100 and the other 200, so older closes were silently dropped.
pub const TRADE_HISTORY_LIMIT: usize = 500;

/// Persistent PnL data that survives restarts
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PersistentPnlData {
    pub total_realized_pnl: f64,
    pub initial_equity: f64,
    pub peak_equity: f64,
    pub equity_history: Vec<(i64, f64)>,
    pub pnl_history: Vec<(i64, f64)>,
    pub trade_history: Vec<serde_json::Value>,
    /// Per-day realized PnL: key = "YYYY-MM-DD", value = realized pnl that day
    pub daily_pnl_map: std::collections::HashMap<String, f64>,
    /// Lifetime notional volume Σ|price × quantity| across every recorded fill.
    /// Survives the trade-history ring buffer so the History card does not
    /// under-report after old fills are dropped.
    #[serde(default)]
    pub total_volume: f64,
    /// Lifetime count of close events (Partial/Full/Stop/…).
    #[serde(default)]
    pub total_closed_trades: u64,
}

impl PersistentPnlData {
    pub fn load(network: &str) -> Option<Self> {
        let path = super::runtime_paths::data_file(network, PNL_STATE_FILE).ok()?;
        let data = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&data) {
            Ok(state) => {
                info!("📂 Loaded PnL state from {}", path.display());
                Some(state)
            }
            Err(e) => {
                warn!("⚠️ Failed to parse PnL state file: {}", e);
                None
            }
        }
    }

    pub fn save(&self, network: &str) {
        let Ok(path) = super::runtime_paths::data_file(network, PNL_STATE_FILE) else {
            warn!("⚠️ Refusing to save PnL for invalid network {network}");
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    warn!("⚠️ Failed to save PnL state: {}", e);
                }
            }
            Err(e) => warn!("⚠️ Failed to serialize PnL state: {}", e),
        }
    }
}

const STRATEGY_CONFIG_FILE: &str = "strategy_config.json";
const RISK_CONFIG_FILE: &str = "risk_config.json";

/// Persistent strategy configuration that survives restarts
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PersistentStrategyConfig {
    pub strategy_name: String,
    pub strategy_params: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub universe_mode: String,
}

impl PersistentStrategyConfig {
    pub fn save(ds: &DashboardState) {
        let Ok(path) = super::runtime_paths::data_file(&ds.network_name, STRATEGY_CONFIG_FILE)
        else {
            warn!(
                "⚠️ Refusing to save strategy for invalid network {}",
                ds.network_name
            );
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let config = PersistentStrategyConfig {
            strategy_name: ds.strategy_name.clone(),
            strategy_params: ds.strategy_params.clone(),
            universe_mode: ds.universe_mode.clone(),
        };
        match serde_json::to_string_pretty(&config) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    warn!("⚠️ Failed to save strategy config: {}", e);
                }
            }
            Err(e) => warn!("⚠️ Failed to serialize strategy config: {}", e),
        }
    }

    pub fn load(network: &str) -> Option<Self> {
        let path = super::runtime_paths::data_file(network, STRATEGY_CONFIG_FILE).ok()?;
        let data = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&data) {
            Ok(config) => {
                info!("📂 Loaded strategy config from {}", path.display());
                Some(config)
            }
            Err(e) => {
                warn!("⚠️ Failed to parse strategy config file: {}", e);
                None
            }
        }
    }
}

/// Persistent risk configuration that survives restarts
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PersistentRiskConfig {
    pub risk_config: serde_json::Value,
    pub leverage_limit: f64,
}

impl PersistentRiskConfig {
    pub fn save(ds: &DashboardState) {
        let Ok(path) = super::runtime_paths::data_file(&ds.network_name, RISK_CONFIG_FILE) else {
            warn!(
                "⚠️ Refusing to save risk config for invalid network {}",
                ds.network_name
            );
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let config = PersistentRiskConfig {
            risk_config: ds.risk_config.clone(),
            leverage_limit: ds.leverage_limit,
        };
        match serde_json::to_string_pretty(&config) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&path, json) {
                    warn!("⚠️ Failed to save risk config: {}", e);
                }
            }
            Err(e) => warn!("⚠️ Failed to serialize risk config: {}", e),
        }
    }

    pub fn load(network: &str) -> Option<Self> {
        let path = super::runtime_paths::data_file(network, RISK_CONFIG_FILE).ok()?;
        let data = std::fs::read_to_string(&path).ok()?;
        match serde_json::from_str(&data) {
            Ok(config) => {
                info!("📂 Loaded risk config from {}", path.display());
                Some(config)
            }
            Err(e) => {
                warn!("⚠️ Failed to parse risk config file: {}", e);
                None
            }
        }
    }
}

/// Shared dashboard state
#[derive(Clone, Default)]
pub struct DashboardState {
    /// Network used by this process. Switching profiles requires a restart.
    pub network_name: String,
    pub rest_url: String,
    pub ws_url: String,
    pub chain_id: i32,
    pub equity: f64,
    pub available_balance: f64,
    pub unrealized_pnl: f64,
    pub strategy_name: String,
    pub total_trades: u64,
    pub open_orders: u32,
    pub open_orders_list: Vec<serde_json::Value>,
    pub positions: Vec<serde_json::Value>,
    pub trade_history: Vec<serde_json::Value>,
    pub event_history: Vec<super::event_log::DashboardEvent>,
    pub risk_status: Option<serde_json::Value>,
    // PnL tracking
    pub daily_realized_pnl: f64,
    pub total_realized_pnl: f64,
    pub initial_equity: f64,
    pub peak_equity: f64,
    pub equity_history: Vec<(i64, f64)>, // (unix_ts, equity) — for chart
    pub pnl_history: Vec<(i64, f64)>,    // (unix_ts, cumulative_pnl)
    /// Lifetime notional volume Σ|price × quantity| (survives ring-buffer trim).
    pub total_volume: f64,
    /// Lifetime close-event count (survives ring-buffer trim).
    pub total_closed_trades: u64,
    // Strategy config (can be modified from dashboard)
    pub strategy_params: std::collections::HashMap<String, String>,
    pub strategy_config_changed: bool,
    /// `explicit` (checkbox list) or `auto` (rank every discovered perp).
    pub universe_mode: String,
    // Per-day PnL tracking (persisted)
    pub daily_pnl_map: std::collections::HashMap<String, f64>,
    // Trading controls (runtime)
    pub active_markets: Vec<u32>,   // Markets currently being traded
    pub trading_paused: bool,       // Pause all trading signals
    pub cancel_all_requested: bool, // Request to cancel all open orders
    pub available_markets: Vec<(u32, String)>, // All known markets: (id, symbol)
    // Risk config (runtime-editable from dashboard)
    pub risk_config: serde_json::Value, // Cached risk config for display
    pub risk_update_requested: Option<serde_json::Value>, // Pending risk update
    pub leverage_limit: f64,            // Runtime leverage limit (used by main loop)
    /// symbol -> 最新盘口中间价。由主循环每个 tick 从 `snapshot.order_books` 注入。
    /// 独立于 positions：空仓时 positions 里没有任何价格，面板就没有行情可显示。
    pub last_prices: std::collections::HashMap<String, f64>,
    /// Server-side proposal ledger. The model cannot mutate live strategy state directly.
    pub quant_agent: super::quant_agent::AgentLedger,
}

impl DashboardState {
    /// Save current PnL state to disk
    pub fn save_pnl(&self) {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut daily_map = self.daily_pnl_map.clone();
        daily_map.insert(today, self.daily_realized_pnl);

        let persistent = PersistentPnlData {
            total_realized_pnl: self.total_realized_pnl,
            initial_equity: self.initial_equity,
            peak_equity: self.peak_equity,
            equity_history: self.equity_history.clone(),
            pnl_history: self.pnl_history.clone(),
            trade_history: self.trade_history.clone(),
            daily_pnl_map: daily_map,
            total_volume: self.total_volume,
            total_closed_trades: self.total_closed_trades,
        };
        persistent.save(&self.network_name);
    }

    /// Restore PnL state from persistent data
    pub fn restore_pnl(&mut self, data: &PersistentPnlData) {
        self.total_realized_pnl = data.total_realized_pnl;
        // Only restore initial_equity if it was set (non-zero)
        if data.initial_equity > 0.0 {
            self.initial_equity = data.initial_equity;
        }
        if data.peak_equity > self.peak_equity {
            self.peak_equity = data.peak_equity;
        }
        // Merge equity history: keep persisted + add current
        if !data.equity_history.is_empty() {
            self.equity_history = data.equity_history.clone();
        }
        if !data.pnl_history.is_empty() {
            self.pnl_history = data.pnl_history.clone();
        }
        // Restore trade history
        if !data.trade_history.is_empty() {
            self.trade_history = data.trade_history.clone();
        }
        // Lifetime volume / close counts: prefer persisted values; if missing
        // (old state files), recompute from the retained buffer as a floor.
        let (buf_vol, buf_closes) = Self::stats_from_trades(&self.trade_history);
        self.total_volume = if data.total_volume > 0.0 {
            data.total_volume
        } else {
            buf_vol
        };
        self.total_closed_trades = if data.total_closed_trades > 0 {
            data.total_closed_trades
        } else {
            buf_closes
        };
        // Restore today's daily PnL
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        self.daily_realized_pnl = data.daily_pnl_map.get(&today).copied().unwrap_or(0.0);
        self.daily_pnl_map = data.daily_pnl_map.clone();
        info!(
            "📂 Restored PnL: total={:.4}, daily={:.4}, peak={:.2}, trades={}, volume={:.2}, closed={}",
            self.total_realized_pnl,
            self.daily_realized_pnl,
            self.peak_equity,
            self.trade_history.len(),
            self.total_volume,
            self.total_closed_trades
        );
    }

    /// Notional volume and close-event count from a trade list.
    pub fn stats_from_trades(trades: &[serde_json::Value]) -> (f64, u64) {
        let mut volume = 0.0_f64;
        let mut closes = 0_u64;
        for t in trades {
            let price = t.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let qty = t.get("quantity").and_then(|v| v.as_f64()).unwrap_or(0.0);
            volume += (price * qty).abs();
            let action = t
                .get("action")
                .or_else(|| t.get("close_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if action_is_close(action) {
                closes += 1;
            }
        }
        (volume, closes)
    }

    /// Append a fill to the ring buffer and update lifetime volume/close counters.
    pub fn push_trade(&mut self, trade: serde_json::Value) {
        let price = trade.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let qty = trade
            .get("quantity")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        self.total_volume += (price * qty).abs();
        let action = trade
            .get("action")
            .or_else(|| trade.get("close_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if action_is_close(action) {
            self.total_closed_trades += 1;
        }
        self.trade_history.push(trade);
        let len = self.trade_history.len();
        if len > TRADE_HISTORY_LIMIT {
            self.trade_history.drain(..len - TRADE_HISTORY_LIMIT);
        }
    }
}

fn action_is_close(action: &str) -> bool {
    let lower = action.to_ascii_lowercase();
    lower.contains("close")
        || lower.contains("stop")
        || lower.contains("emergency")
        || lower.contains("liquidat")
}

pub type SharedDashboardState = Arc<RwLock<DashboardState>>;

/// Start Dashboard Web server
pub async fn start(host: &str, port: u16) -> Result<()> {
    let state: SharedDashboardState = Arc::new(RwLock::new(DashboardState {
        network_name: crate::env_profiles::selected_network(),
        ..DashboardState::default()
    }));
    start_with_state(host, port, state).await
}

pub async fn start_with_state(host: &str, port: u16, state: SharedDashboardState) -> Result<()> {
    super::event_log::restore_event_history(&state).await;
    super::event_log::spawn_event_monitor(state.clone());

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/ai", get(ai_page_handler))
        .route("/ai.js", get(ai_js_handler))
        .route("/quant_agent.js", get(quant_agent_js_handler))
        .route(
            "/quant_agent_protocol.js",
            get(quant_agent_protocol_js_handler),
        )
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .route("/api/status", get(status_handler))
        .route("/api/positions", get(positions_handler))
        .route("/api/trades", get(trades_handler))
        .route("/api/events", get(events_handler))
        .route("/api/env", get(env_get_handler))
        .route("/api/env", post(env_update_handler))
        .route("/api/network", get(network_get_handler))
        .route("/api/network", post(network_update_handler))
        .route("/api/pnl", get(pnl_handler))
        .route("/api/strategy", get(strategy_get_handler))
        .route("/api/strategy", post(strategy_update_handler))
        .route("/api/backtest", post(backtest_handler))
        .route("/api/backtest/datasets", get(backtest_datasets_handler))
        .route("/api/backtest/optimize", post(backtest_optimize_handler))
        .route("/api/agent/status", get(agent_status_handler))
        .route("/api/agent/audit", get(agent_audit_handler))
        .route("/api/agent/proposals", post(agent_proposal_handler))
        .route("/api/agent/proposals/:id/apply", post(agent_apply_handler))
        .route(
            "/api/backtest/opencode-optimize",
            post(opencode_optimize_handler),
        )
        .route("/api/trading/markets", get(markets_get_handler))
        .route("/api/trading/markets", post(markets_update_handler))
        .route("/api/trading/pause", post(trading_pause_handler))
        .route("/api/trading/resume", post(trading_resume_handler))
        .route("/api/trading/cancel-all", post(cancel_all_handler))
        .route("/api/risk/config", get(risk_config_get_handler))
        .route("/api/risk/config", post(risk_config_update_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    info!("Dashboard running at: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn index_handler() -> Html<&'static str> {
    Html(include_str!("ui/index.html"))
}

async fn js_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("ui/app.js"),
    )
}

async fn health_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "uptime": "running"
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedDashboardState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state))
}

async fn handle_ws_connection(mut socket: WebSocket, state: SharedDashboardState) {
    info!("New dashboard WebSocket connection");

    let welcome = serde_json::json!({
        "type": "welcome",
        "message": "Connected to Lighter Bot Dashboard"
    });
    let _ = socket.send(Message::Text(welcome.to_string())).await;

    // Auto-push state every 3 seconds alongside handling client requests
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let state_push = state.clone();
    let push_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(3));
        loop {
            interval.tick().await;
            let ds = state_push.read().await;
            let status_msg = serde_json::json!({
                "type": "status",
                "data": {
                    "running": true,
                    "strategy": ds.strategy_name,
                    "total_trades": ds.total_trades,
                    "equity": ds.equity,
                    "unrealized_pnl": ds.unrealized_pnl,
                    "available_balance": ds.available_balance,
                    "open_orders": ds.open_orders,
                    "inception_pnl": super::pnl_accounting::inception_pnl(
                        ds.equity,
                        ds.initial_equity,
                    ),
                    "daily_pnl": super::pnl_accounting::daily_mark_to_market(
                        ds.equity,
                        super::pnl_accounting::start_of_day_equity(
                            &ds.equity_history,
                            chrono::Utc::now().timestamp(),
                            ds.equity,
                        ),
                    ),
                    "daily_realized_pnl": ds.daily_realized_pnl,
                    "total_realized_pnl": ds.total_realized_pnl,
                    "total_volume": ds.total_volume,
                    "total_closed_trades": ds.total_closed_trades,
                    "initial_equity": ds.initial_equity,
                    "peak_equity": ds.peak_equity,
                    "total_return_pct": if ds.initial_equity > 0.0 {
                        (ds.equity - ds.initial_equity) / ds.initial_equity * 100.0
                    } else { 0.0 },
                    "trading_paused": ds.trading_paused,
                    "active_markets": ds.active_markets,
                    "universe_mode": ds.universe_mode,
                    "last_prices": ds.last_prices,
                }
            });
            let positions_msg = serde_json::json!({
                "type": "positions",
                "data": ds.positions
            });
            let risk_msg = {
                let risk = ds.risk_status.clone().unwrap_or(serde_json::json!({
                    "drawdown_pct": 0.0,
                    "daily_loss_pct": 0.0,
                    "max_drawdown_limit": 15.0,
                    "daily_loss_limit": 8.0,
                    "is_healthy": true
                }));
                serde_json::json!({ "type": "risk", "data": risk })
            };
            let trades_msg = serde_json::json!({
                "type": "recent_trades",
                "data": ds.trade_history.iter().rev().take(20).collect::<Vec<_>>()
            });
            let events_msg = serde_json::json!({
                "type": "events",
                "data": ds.event_history.iter().rev().collect::<Vec<_>>()
            });
            let orders_msg = serde_json::json!({
                "type": "open_orders",
                "data": ds.open_orders_list
            });
            drop(ds);

            if ws_sender
                .send(Message::Text(status_msg.to_string()))
                .await
                .is_err()
            {
                break;
            }
            let _ = ws_sender
                .send(Message::Text(positions_msg.to_string()))
                .await;
            let _ = ws_sender.send(Message::Text(risk_msg.to_string())).await;
            let _ = ws_sender.send(Message::Text(trades_msg.to_string())).await;
            let _ = ws_sender.send(Message::Text(events_msg.to_string())).await;
            let _ = ws_sender.send(Message::Text(orders_msg.to_string())).await;
        }
    });

    // Handle incoming client requests (orderbook queries, etc.)
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(_text)) => {
                // Client-initiated requests are handled by the auto-push above.
                // Only orderbook needs client request (to specify market_id).
                // The push task handles status/positions/risk automatically.
            }
            Ok(axum::extract::ws::Message::Close(_)) => {
                info!("Dashboard WebSocket closed");
                break;
            }
            Err(e) => {
                tracing::error!("Dashboard WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    push_handle.abort();
}

async fn status_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "status": "running",
        "version": env!("CARGO_PKG_VERSION"),
        "strategy": ds.strategy_name,
        "total_trades": ds.total_trades,
        "equity": ds.equity,
        "total_pnl": super::pnl_accounting::inception_pnl(ds.equity, ds.initial_equity),
        "inception_pnl": super::pnl_accounting::inception_pnl(ds.equity, ds.initial_equity),
        "daily_pnl": super::pnl_accounting::daily_mark_to_market(
            ds.equity,
            super::pnl_accounting::start_of_day_equity(
                &ds.equity_history,
                chrono::Utc::now().timestamp(),
                ds.equity,
            ),
        ),
        "daily_realized_pnl": ds.daily_realized_pnl,
        "total_realized_pnl": ds.total_realized_pnl,
        "unrealized_pnl": ds.unrealized_pnl,
        "initial_equity": ds.initial_equity,
        "last_prices": ds.last_prices,
        "network": ds.network_name,
        "rest_url": ds.rest_url,
        "ws_url": ds.ws_url,
        "chain_id": ds.chain_id,
    }))
}

async fn positions_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "positions": ds.positions
    }))
}

async fn trades_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "trades": ds.trade_history
    }))
}

/// `.env` 里可以从面板编辑的键。
///
/// **LIGHTER_SECRET_KEY 是只写的** —— 它是交易所 API 私钥，拿到就能操作账户。
/// GET 只回报"是否已配置 / 长度 / 后 4 位"，永远不回明文；POST 允许覆盖。
/// 面板本身没有鉴权（见 main.rs 里写死的 "0.0.0.0"），所以把明文回给前端
/// 等于把账户控制权挂在任何能访问该端口的人面前。
const ENV_CREDENTIAL_PUBLIC_KEYS: [&str; 2] = ["LIGHTER_ACCOUNT_INDEX", "LIGHTER_API_KEY_INDEX"];
const ENV_SHARED_PUBLIC_KEYS: [&str; 2] = ["RUST_LOG", "TOKIO_WORKER_THREADS"];
const ENV_SECRET_KEYS: [&str; 1] = ["LIGHTER_SECRET_KEY"];

fn shared_env_file_path() -> std::path::PathBuf {
    std::path::PathBuf::from(".env")
}

fn selected_credential_env_path() -> std::path::PathBuf {
    crate::env_profiles::credential_env_path_for_network(&crate::env_profiles::selected_network())
        .unwrap_or_else(|_| std::path::PathBuf::from(".env"))
}

fn selected_credential_key(key: &str) -> String {
    let suffix = key.strip_prefix("LIGHTER_").unwrap_or(key);
    crate::env_profiles::profile_for_network(&crate::env_profiles::selected_network())
        .unwrap_or(crate::env_profiles::CredentialProfile::Mainnet)
        .env_key(suffix)
}

async fn network_get_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    let selected = crate::env_profiles::selected_network();
    axum::Json(serde_json::json!({
        "active": ds.network_name,
        "selected": selected,
        "rest_url": ds.rest_url,
        "ws_url": ds.ws_url,
        "chain_id": ds.chain_id,
        "requires_restart": true,
        "profiles": {
            "mainnet": {
                "label": "Lighter Mainnet", "quote_asset": "USDC",
                "rest_url": "https://mainnet.zklighter.elliot.ai",
                "ws_url": "wss://mainnet.zklighter.elliot.ai/stream", "chain_id": 304
            },
            "robinhood": {
                "label": "Robinhood Chain", "quote_asset": "USDG",
                "rest_url": "https://api.rh.lighter.xyz",
                "ws_url": "wss://api.rh.lighter.xyz/stream", "chain_id": 466324
            }
        }
    }))
}

async fn network_update_handler(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let network = body.get("network").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(network, "mainnet" | "robinhood") {
        return axum::Json(serde_json::json!({
            "status":"error", "message":"network must be mainnet or robinhood"
        }));
    }
    let updates =
        std::collections::HashMap::from([("LIGHTER_NETWORK".to_string(), network.to_string())]);
    match write_env_keys_to(&shared_env_file_path(), &updates) {
        Ok(()) => axum::Json(serde_json::json!({
            "status":"ok", "network":network, "requires_restart":true
        })),
        Err(e) => axum::Json(serde_json::json!({"status":"error","message":e.to_string()})),
    }
}

/// 保留注释与未知键，只替换已知键的值；文件不存在时新建。
fn write_env_keys_to(
    path: &std::path::Path,
    updates: &std::collections::HashMap<String, String>,
) -> std::io::Result<()> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut out = String::new();

    for line in original.lines() {
        let trimmed = line.trim_start();
        // 注释和空行原样保留
        if trimmed.starts_with('#') || trimmed.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        match trimmed.split_once('=') {
            Some((k, _)) if updates.contains_key(k.trim()) => {
                let key = k.trim();
                out.push_str(&format!("{}={}\n", key, updates[key]));
                seen.insert(updates.get_key_value(key).unwrap().0.as_str());
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    // 文件里原本没有的键追加到末尾
    let missing: Vec<_> = updates
        .keys()
        .filter(|k| !seen.contains(k.as_str()))
        .collect();
    if !missing.is_empty() {
        out.push_str("\n# —— 由面板写入 ——\n");
        for k in missing {
            out.push_str(&format!("{}={}\n", k, updates[k]));
        }
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(out.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, out)
    }
}

async fn env_get_handler() -> impl IntoResponse {
    let credential_path = selected_credential_env_path();
    let shared_path = shared_env_file_path();
    let mut public = serde_json::Map::new();
    for key in ENV_CREDENTIAL_PUBLIC_KEYS {
        let stored_key = selected_credential_key(key);
        public.insert(
            key.to_string(),
            serde_json::json!(
                crate::env_profiles::read_env_value(&credential_path, &stored_key)
                    .unwrap_or_default()
            ),
        );
    }
    for key in ENV_SHARED_PUBLIC_KEYS {
        public.insert(
            key.to_string(),
            serde_json::json!(
                crate::env_profiles::read_env_value(&shared_path, key).unwrap_or_default()
            ),
        );
    }
    let mut secrets = serde_json::Map::new();
    for key in ENV_SECRET_KEYS {
        let stored_key = selected_credential_key(key);
        let val =
            crate::env_profiles::read_env_value(&credential_path, &stored_key).unwrap_or_default();
        secrets.insert(
            key.to_string(),
            serde_json::json!({
                "configured": !val.is_empty(),
                "length": val.len(),
                // 只回后 4 位，够核对"是不是我以为的那把"，又不足以复原
                "tail": if val.len() >= 4 { val[val.len() - 4..].to_string() } else { String::new() },
            }),
        );
    }
    axum::Json(serde_json::json!({
        "public": public,
        "secrets": secrets,
        "env_path": credential_path.to_string_lossy(),
        // 环境变量在进程启动时读入，改完必须重启才会生效
        "requires_restart": true,
    }))
}

async fn env_update_handler(axum::Json(body): axum::Json<serde_json::Value>) -> impl IntoResponse {
    let Some(obj) = body.as_object() else {
        return axum::Json(
            serde_json::json!({"status":"error","message":"body must be an object"}),
        );
    };
    let mut updates = std::collections::HashMap::new();
    let mut rejected = Vec::new();
    for (k, v) in obj {
        let allowed = ENV_CREDENTIAL_PUBLIC_KEYS.contains(&k.as_str())
            || ENV_SHARED_PUBLIC_KEYS.contains(&k.as_str())
            || ENV_SECRET_KEYS.contains(&k.as_str());
        if !allowed {
            rejected.push(k.clone());
            continue;
        }
        let val = v.as_str().unwrap_or_default().trim().to_string();
        // 空字符串视为"不修改"，避免前端把掩码占位符写回去把密钥清空
        if val.is_empty() {
            continue;
        }
        updates.insert(k.clone(), val);
    }
    if updates.is_empty() {
        return axum::Json(serde_json::json!({
            "status":"error","message":"no writable keys in request","rejected":rejected}));
    }
    let credential_updates: std::collections::HashMap<_, _> = updates
        .iter()
        .filter(|(key, _)| {
            ENV_CREDENTIAL_PUBLIC_KEYS.contains(&key.as_str())
                || ENV_SECRET_KEYS.contains(&key.as_str())
        })
        .map(|(key, value)| (selected_credential_key(key), value.clone()))
        .collect();
    let shared_updates: std::collections::HashMap<_, _> = updates
        .iter()
        .filter(|(key, _)| ENV_SHARED_PUBLIC_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let credential_path = selected_credential_env_path();
    let write_result = if !credential_updates.is_empty() {
        write_env_keys_to(&credential_path, &credential_updates)
    } else {
        Ok(())
    }
    .and_then(|()| {
        if shared_updates.is_empty() {
            Ok(())
        } else {
            write_env_keys_to(&shared_env_file_path(), &shared_updates)
        }
    });
    match write_result {
        Ok(()) => {
            let keys: Vec<_> = updates.keys().cloned().collect();
            info!("环境变量已写入网络隔离配置: {:?}（重启后生效）", keys);
            axum::Json(serde_json::json!({
                "status":"ok",
                "updated": keys,
                "rejected": rejected,
                "requires_restart": true,
                "env_path": credential_path.to_string_lossy(),
                "message":"Saved to the selected network profile. Restart the bot for changes to take effect."
            }))
        }
        Err(e) => axum::Json(serde_json::json!({"status":"error","message":e.to_string()})),
    }
}

async fn events_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "events": ds.event_history.iter().rev().collect::<Vec<_>>()
    }))
}

async fn pnl_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    // Prefer lifetime counters; fall back to the retained buffer so old
    // processes without these fields still return something sensible.
    let (buf_vol, buf_closes) = DashboardState::stats_from_trades(&ds.trade_history);
    let total_volume = if ds.total_volume > 0.0 {
        ds.total_volume
    } else {
        buf_vol
    };
    let total_closed_trades = if ds.total_closed_trades > 0 {
        ds.total_closed_trades
    } else {
        buf_closes
    };
    axum::Json(serde_json::json!({
        "daily_realized_pnl": ds.daily_realized_pnl,
        "total_realized_pnl": ds.total_realized_pnl,
        "inception_pnl": super::pnl_accounting::inception_pnl(ds.equity, ds.initial_equity),
        "daily_pnl": super::pnl_accounting::daily_mark_to_market(
            ds.equity,
            super::pnl_accounting::start_of_day_equity(
                &ds.equity_history,
                chrono::Utc::now().timestamp(),
                ds.equity,
            ),
        ),
        "unrealized_pnl": ds.unrealized_pnl,
        "equity": ds.equity,
        "initial_equity": ds.initial_equity,
        "peak_equity": ds.peak_equity,
        "total_volume": total_volume,
        "total_closed_trades": total_closed_trades,
        "trade_history_limit": TRADE_HISTORY_LIMIT,
        "trade_history_len": ds.trade_history.len(),
        "total_return_pct": if ds.initial_equity > 0.0 {
            (ds.equity - ds.initial_equity) / ds.initial_equity * 100.0
        } else { 0.0 },
        "equity_history": ds.equity_history.iter()
            .map(|(ts, eq)| serde_json::json!({"t": ts, "v": eq}))
            .collect::<Vec<_>>(),
        "pnl_history": ds.pnl_history.iter()
            .map(|(ts, pnl)| serde_json::json!({"t": ts, "v": pnl}))
            .collect::<Vec<_>>(),
        "daily_pnl_map": ds.daily_pnl_map,
        // Newest first; full retained buffer (limit is TRADE_HISTORY_LIMIT).
        "trades": ds.trade_history.iter().rev().collect::<Vec<_>>(),
    }))
}

async fn strategy_get_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "strategy": ds.strategy_name,
        "params": ds.strategy_params,
    }))
}

fn agent_policy(ds: &DashboardState) -> super::quant_agent::PolicySnapshot {
    let emergency_triggered = ds
        .risk_status
        .as_ref()
        .and_then(|risk| risk.get("emergency_triggered"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let max_drawdown_pct = ds
        .risk_config
        .get("max_drawdown_pct")
        .and_then(|value| value.as_f64())
        .unwrap_or(10.0)
        .clamp(0.1, 25.0);
    super::quant_agent::PolicySnapshot {
        equity: ds.equity,
        trading_paused: ds.trading_paused,
        emergency_triggered,
        max_drawdown_pct,
        // This is deliberately a backend constant, not a model-supplied parameter.
        max_notional_pct: 25.0,
    }
}

async fn agent_status_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    let policy = agent_policy(&ds);
    let pending = ds
        .quant_agent
        .proposals
        .iter()
        .filter(|proposal| proposal.status == "pending")
        .count();
    axum::Json(serde_json::json!({
        "status": if policy.trading_paused || policy.emergency_triggered { "blocked" } else { "ready" },
        "mode": "proposal_only",
        "model_authority": "research_and_propose",
        "execution_authority": "human_approval_required",
        "policy": policy,
        "pending_proposals": pending,
    }))
}

async fn agent_audit_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    let records = ds
        .quant_agent
        .proposals
        .iter()
        .rev()
        .take(50)
        .collect::<Vec<_>>();
    axum::Json(serde_json::json!({"status":"ok", "records": records}))
}

async fn agent_proposal_handler(
    State(state): State<SharedDashboardState>,
    axum::Json(mut input): axum::Json<super::quant_agent::ProposalInput>,
) -> impl IntoResponse {
    // Evidence supplied by a model/browser is untrusted. Re-run the exact candidate on
    // the server and replace every metric before the deterministic policy sees it.
    let mut param_pairs = input
        .params
        .iter()
        .map(|(key, value)| {
            let value = value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string());
            format!("{key}={value}")
        })
        .collect::<Vec<_>>();
    param_pairs.sort();
    let params = param_pairs.join(",");
    let verified = match run_backtest_result(
        &input.strategy,
        &params,
        &input.evidence.data_file,
        input.evidence.capital,
        &input.evidence.start,
        &input.evidence.end,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            return axum::Json(serde_json::json!({
                "status":"error", "message":format!("server verification backtest failed: {error}")
            }))
        }
    };
    input.evidence.total_return_pct = verified["total_return_pct"].as_f64().unwrap_or(f64::NAN);
    input.evidence.sharpe_ratio = verified["sharpe_ratio"].as_f64().unwrap_or(f64::NAN);
    input.evidence.max_drawdown_pct = verified["max_drawdown_pct"].as_f64().unwrap_or(f64::NAN);
    input.evidence.total_trades = verified["total_trades"].as_u64().unwrap_or(0);

    let mut ds = state.write().await;
    let policy = agent_policy(&ds);
    let decision = super::quant_agent::evaluate_proposal(&input, &policy);
    let id: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(18)
        .map(char::from)
        .collect();
    let approval_phrase = format!("APPLY {id}");
    let proposal = super::quant_agent::AgentProposal {
        id,
        created_at: chrono::Utc::now().to_rfc3339(),
        status: if decision.allowed {
            "pending"
        } else {
            "rejected"
        }
        .to_string(),
        input,
        policy,
        decision,
        approval_phrase,
    };
    ds.quant_agent.record(proposal.clone());
    if let Err(error) = ds.quant_agent.save(&ds.network_name) {
        warn!("Failed to persist Quant Agent audit: {}", error);
    }
    axum::Json(serde_json::json!({"status":"ok", "proposal": proposal}))
}

#[derive(Deserialize)]
struct AgentApproval {
    approval_phrase: String,
}

async fn agent_apply_handler(
    State(state): State<SharedDashboardState>,
    Path(id): Path<String>,
    axum::Json(approval): axum::Json<AgentApproval>,
) -> impl IntoResponse {
    let mut ds = state.write().await;
    let Some(index) = ds
        .quant_agent
        .proposals
        .iter()
        .position(|proposal| proposal.id == id)
    else {
        return axum::Json(serde_json::json!({"status":"error","message":"proposal not found"}));
    };

    let proposal = ds.quant_agent.proposals[index].clone();
    if proposal.status != "pending" || approval.approval_phrase != proposal.approval_phrase {
        return axum::Json(serde_json::json!({
            "status":"error",
            "message":"proposal is not pending or approval phrase does not match"
        }));
    }
    // Re-evaluate against current runtime state so stale approvals cannot bypass a new pause/emergency.
    let current_policy = agent_policy(&ds);
    let current_decision = super::quant_agent::evaluate_proposal(&proposal.input, &current_policy);
    if !current_decision.allowed {
        ds.quant_agent.proposals[index].status = "blocked_at_apply".to_string();
        ds.quant_agent.proposals[index].decision = current_decision.clone();
        let _ = ds.quant_agent.save(&ds.network_name);
        return axum::Json(serde_json::json!({
            "status":"error", "message":"current policy blocked apply", "decision":current_decision
        }));
    }

    ds.strategy_name = match proposal.input.strategy.as_str() {
        "grid" => "grid_trading",
        "trend" => "trend_following",
        "mm" => "market_making",
        other => other,
    }
    .to_string();
    ds.strategy_params = proposal
        .input
        .params
        .iter()
        .map(|(key, value)| {
            let normalized = value
                .as_str()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| value.to_string());
            (key.clone(), normalized)
        })
        .collect();
    ds.strategy_config_changed = true;
    ds.quant_agent.proposals[index].status = "applied".to_string();
    PersistentStrategyConfig::save(&ds);
    if let Err(error) = ds.quant_agent.save(&ds.network_name) {
        warn!("Failed to persist Quant Agent apply audit: {}", error);
    }
    info!(
        "Quant Agent proposal {} applied after explicit approval",
        id
    );
    axum::Json(serde_json::json!({
        "status":"ok", "message":"approved proposal applied", "proposal_id":id,
        "strategy":ds.strategy_name, "params":ds.strategy_params
    }))
}

async fn strategy_update_handler(
    State(state): State<SharedDashboardState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let mut ds = state.write().await;
    // Allow switching strategy name
    if let Some(name) = body.get("strategy").and_then(|s| s.as_str()) {
        ds.strategy_name = name.to_string();
        ds.strategy_config_changed = true;
        info!("Strategy switched to: {}", name);
    }
    if let Some(params) = body.get("params").and_then(|p| p.as_object()) {
        for (k, v) in params {
            ds.strategy_params.insert(
                k.clone(),
                v.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| v.as_f64().map(|n| n.to_string()))
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                    .unwrap_or_default(),
            );
        }
        ds.strategy_config_changed = true;
        info!(
            "Strategy params updated from dashboard: {:?}",
            ds.strategy_params
        );
    }
    PersistentStrategyConfig::save(&ds);
    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Strategy config updated. Changes will apply shortly.",
        "strategy": ds.strategy_name,
        "params": ds.strategy_params,
    }))
}

/// List CSVs under `backtests/data/` with their actual first/last candle dates
/// so the AI Lab date pickers stay aligned with real data (not hardcoded months).
async fn backtest_datasets_handler() -> impl IntoResponse {
    let dir = std::path::Path::new("backtests/data");
    let mut items: Vec<serde_json::Value> = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("Cannot read backtests/data: {}", e),
                "datasets": []
            }));
        }
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("csv") {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let path_str = path.to_string_lossy().to_string();
        match crate::data::loader::load_csv_data(&path_str) {
            Ok(candles) if !candles.is_empty() => {
                let start = candles.first().unwrap().timestamp.date_naive().to_string();
                let end = candles.last().unwrap().timestamp.date_naive().to_string();
                items.push(serde_json::json!({
                    "file": name,
                    "start": start,
                    "end": end,
                    "candles": candles.len(),
                    "label": format!("{} ({} → {}, {} bars)", name, start, end, candles.len()),
                }));
            }
            Ok(_) => {
                items.push(serde_json::json!({
                    "file": name,
                    "start": serde_json::Value::Null,
                    "end": serde_json::Value::Null,
                    "candles": 0,
                    "label": format!("{} (empty)", name),
                }));
            }
            Err(e) => {
                warn!("Skip dataset {}: {}", name, e);
            }
        }
    }
    // Newest end date first, then longest series — preferred default is recent mainnet.
    items.sort_by(|a, b| {
        let ae = a.get("end").and_then(|v| v.as_str()).unwrap_or("");
        let be = b.get("end").and_then(|v| v.as_str()).unwrap_or("");
        be.cmp(ae).then_with(|| {
            let ac = a.get("candles").and_then(|v| v.as_u64()).unwrap_or(0);
            let bc = b.get("candles").and_then(|v| v.as_u64()).unwrap_or(0);
            bc.cmp(&ac)
        })
    });
    let default_file = items
        .first()
        .and_then(|d| d.get("file"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    axum::Json(serde_json::json!({
        "status": "ok",
        "datasets": items,
        "default": default_file,
        "today": chrono::Utc::now().date_naive().to_string(),
    }))
}

async fn backtest_handler(axum::Json(body): axum::Json<serde_json::Value>) -> impl IntoResponse {
    let strategy = body
        .get("strategy")
        .and_then(|s| s.as_str())
        .unwrap_or("grid");
    let params = body.get("params").and_then(|s| s.as_str()).unwrap_or("");
    let data_file = body.get("data_file").and_then(|s| s.as_str()).unwrap_or("");
    let capital = body
        .get("capital")
        .and_then(|c| c.as_f64())
        .unwrap_or(125.0);
    // Accept start/end and the legacy start_date/end_date aliases used by older AI Lab JS.
    let start = body
        .get("start")
        .or_else(|| body.get("start_date"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let end = body
        .get("end")
        .or_else(|| body.get("end_date"))
        .and_then(|s| s.as_str())
        .unwrap_or("");

    match run_backtest_result(strategy, params, data_file, capital, start, end).await {
        Ok(result) => axum::Json(result),
        Err(e) => axum::Json(serde_json::json!({"status": "error", "message": e.to_string()})),
    }
}

/// Trend defaults to notional=$1000 in create_strategy_with_params, but the AI Lab
/// often runs with $125–$200 capital. The engine then silently skips every open
/// (cost > capital) while the strategy still records a phantom position → 0 trades.
/// Clamp / inject an affordable notional for backtests only.
fn normalize_backtest_params(strategy: &str, params: &str, capital: f64) -> String {
    let is_trend = matches!(strategy, "trend" | "trend_following");
    let is_dca = strategy == "dca";
    if !is_trend && !is_dca {
        return params.to_string();
    }

    let mut map: std::collections::BTreeMap<String, String> = params
        .split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let k = parts.next()?.trim();
            let v = parts.next()?.trim();
            if k.is_empty() {
                return None;
            }
            Some((k.to_string(), v.to_string()))
        })
        .collect();

    if is_trend {
        let cap = capital.max(1.0);
        // leave ~5% for fees/slippage headroom
        let max_affordable = (cap * 0.90).max(10.0);
        let default_n = (cap * 0.50).clamp(10.0, max_affordable);
        let n = map
            .get("notional")
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(default_n);
        let floor = 10.0_f64.min(max_affordable);
        let clamped = n.min(max_affordable).max(floor);
        if map.get("notional").map(|s| s.as_str()) != Some(&format!("{clamped}")) {
            if (n - clamped).abs() > 1e-9 {
                warn!(
                    "回测 notional 从 {} 调整为 {:.2}（资金 {:.2}，否则趋势策略会 0 成交）",
                    map.get("notional")
                        .map(|s| s.as_str())
                        .unwrap_or("<default 1000>"),
                    clamped,
                    capital
                );
            }
            map.insert("notional".to_string(), format!("{clamped:.4}"));
        }
    }

    if is_dca {
        let cap = capital.max(1.0);
        let max_amt = (cap * 0.5).max(1.0);
        let amt = map
            .get("amount")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(5.0)
            .min(max_amt)
            .max(1.0);
        map.insert("amount".to_string(), format!("{amt:.4}"));
    }

    map.into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

async fn run_backtest_result(
    strategy: &str,
    params: &str,
    data_file: &str,
    capital: f64,
    start: &str,
    end: &str,
) -> Result<serde_json::Value> {
    if data_file.is_empty() || start.is_empty() || end.is_empty() {
        anyhow::bail!("Missing required fields: data_file, start, end");
    }

    let data_path = if data_file.starts_with('/') || data_file.starts_with("backtests/") {
        data_file.to_string()
    } else {
        format!("backtests/data/{}", data_file)
    };

    let historical_data = crate::data::loader::load_csv_data_in_range(&data_path, start, end)
        .map_err(|e| anyhow::anyhow!("Data load failed: {}", e))?;
    let candle_count = historical_data.len();
    let normalized = normalize_backtest_params(strategy, params, capital);
    let bt_strategy = crate::strategy::create_strategy_with_params(
        strategy,
        if normalized.is_empty() {
            None
        } else {
            Some(normalized.as_str())
        },
    )
    .map_err(|e| anyhow::anyhow!("Invalid strategy: {}", e))?;

    let mut engine = crate::backtest::engine::BacktestEngine::new(capital, historical_data);
    let results = engine
        .run(bt_strategy)
        .await
        .map_err(|e| anyhow::anyhow!("Backtest failed: {}", e))?;

    Ok(serde_json::json!({
        "status": "ok",
        "strategy": strategy,
        "data_file": data_file,
        "params": normalized,
        "candles": candle_count,
        "total_return_pct": results.total_return * 100.0,
        "sharpe_ratio": results.sharpe_ratio,
        "max_drawdown_pct": results.max_drawdown * 100.0,
        "total_trades": results.total_trades,
        "winning_trades": results.winning_trades,
        "losing_trades": results.losing_trades,
        "win_rate_pct": results.win_rate * 100.0,
        "profit_factor": results.profit_factor,
        "avg_profit": results.avg_profit,
        "avg_loss": results.avg_loss,
        "initial_capital": results.initial_capital,
        "final_capital": results.final_capital,
        "equity_curve": results.equity_curve.iter()
            .map(|(ts, eq)| serde_json::json!({"t": ts.timestamp(), "v": eq}))
            .collect::<Vec<_>>(),
        "trades": results.trades.iter().take(100)
            .map(|t| serde_json::json!({
                "timestamp": t.timestamp.to_rfc3339(),
                "symbol": t.symbol,
                "side": format!("{:?}", t.side),
                "price": t.price,
                "quantity": t.quantity,
                "pnl": t.pnl,
                "commission": t.commission,
            }))
            .collect::<Vec<_>>(),
    }))
}

/// Score a sweep row for ranking. Higher is better for all goals.
fn score_optimize_row(goal: &str, ret: f64, sharpe: f64, max_dd: f64, trades: u64) -> f64 {
    if trades == 0 {
        return f64::NEG_INFINITY;
    }
    match goal {
        "return" => ret,
        "drawdown" => -max_dd, // lower drawdown wins
        "balanced" => ret - max_dd * 0.5 + sharpe * 2.0,
        _ => sharpe, // default sharpe
    }
}

fn build_param_grid(strategy: &str, mode: &str, capital: f64) -> Result<Vec<String>> {
    let quick = mode != "full";
    match strategy {
        "grid" | "grid_trading" => {
            let grid_counts: &[i32] = if quick {
                &[6, 8, 10, 12, 16]
            } else {
                &[4, 6, 8, 10, 12, 14, 20]
            };
            // Keep per-grid investment affordable relative to capital.
            let inv_pool: Vec<f64> = if quick {
                vec![8.0, 15.0, 30.0, 50.0]
            } else {
                vec![5.0, 8.0, 12.0, 16.0, 30.0, 50.0]
            };
            let mut investments: Vec<f64> = Vec::new();
            for v in inv_pool {
                let clamped = v.min((capital * 0.4).max(3.0));
                if investments.iter().all(|x| (*x - clamped).abs() > 1e-9) {
                    investments.push(clamped);
                }
            }
            let deviations: &[f64] = if quick {
                &[0.004, 0.008, 0.012, 0.02]
            } else {
                &[0.003, 0.005, 0.008, 0.012, 0.016, 0.02, 0.03]
            };
            let mut sets = Vec::new();
            for &gc in grid_counts {
                for &inv in &investments {
                    for &dev in deviations {
                        sets.push(format!("grid_count={gc},investment={inv},deviation={dev}"));
                    }
                }
            }
            Ok(sets)
        }
        "trend" | "trend_following" => {
            let fast: &[i32] = if quick {
                &[5, 7, 10, 14]
            } else {
                &[5, 7, 10, 14, 21]
            };
            let slow: &[i32] = if quick {
                &[21, 30, 50]
            } else {
                &[14, 21, 30, 50, 80]
            };
            let sls: &[f64] = if quick {
                &[0.03, 0.05]
            } else {
                &[0.02, 0.03, 0.05]
            };
            let tps: &[f64] = if quick {
                &[0.06, 0.10]
            } else {
                &[0.04, 0.06, 0.10]
            };
            // Critical: embed affordable notional so opens can fill under small capital.
            let max_n = (capital * 0.90).max(10.0);
            let notionals: Vec<f64> = if quick {
                vec![
                    (capital * 0.45).clamp(10.0, max_n),
                    (capital * 0.75).clamp(10.0, max_n),
                ]
            } else {
                vec![
                    (capital * 0.30).clamp(10.0, max_n),
                    (capital * 0.50).clamp(10.0, max_n),
                    (capital * 0.80).clamp(10.0, max_n),
                ]
            };
            let mut sets = Vec::new();
            for &f in fast {
                for &s in slow {
                    if f >= s {
                        continue;
                    }
                    for &sl in sls {
                        for &tp in tps {
                            if tp <= sl {
                                continue;
                            }
                            for &n in &notionals {
                                sets.push(format!(
                                    "fast_ma={f},slow_ma={s},stop_loss={sl},take_profit={tp},trailing_stop=0,notional={n:.2}"
                                ));
                            }
                        }
                    }
                }
            }
            Ok(sets)
        }
        "dca" => {
            // DCA has fewer knobs; keep a compact grid.
            let intervals = [2.0, 4.0, 8.0, 12.0];
            let amounts = [5.0, 10.0, 20.0];
            let dips = [0.01, 0.02, 0.03];
            let mut sets = Vec::new();
            for &iv in &intervals {
                for &amt in &amounts {
                    for &dip in &dips {
                        sets.push(format!(
                            "interval={iv},amount={amt},dip_threshold={}",
                            dip * 100.0
                        ));
                    }
                }
            }
            Ok(sets)
        }
        "mm" | "market_making" => {
            let spreads = [0.0005, 0.001, 0.002];
            let notionals = [20.0, 40.0];
            let mut sets = Vec::new();
            for &bid in &spreads {
                for &ask in &spreads {
                    for &n in &notionals {
                        sets.push(format!(
                            "bid_spread={bid},ask_spread={ask},order_notional={n}"
                        ));
                    }
                }
            }
            Ok(sets)
        }
        other => anyhow::bail!("Unsupported strategy for optimize: {other}"),
    }
}

/// Local parameter sweep — no external AI required.
/// Loads candles once, runs many param combos, returns top rows + full best backtest.
async fn backtest_optimize_handler(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let strategy = body
        .get("strategy")
        .and_then(|s| s.as_str())
        .unwrap_or("grid");
    let data_file = body.get("data_file").and_then(|s| s.as_str()).unwrap_or("");
    let capital = body
        .get("capital")
        .and_then(|c| c.as_f64())
        .unwrap_or(125.0);
    let start = body
        .get("start")
        .or_else(|| body.get("start_date"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let end = body
        .get("end")
        .or_else(|| body.get("end_date"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let goal = body
        .get("goal")
        .and_then(|s| s.as_str())
        .unwrap_or("sharpe");
    let mode = body.get("mode").and_then(|s| s.as_str()).unwrap_or("quick");
    let baseline_params = body
        .get("params")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();

    if data_file.is_empty() || start.is_empty() || end.is_empty() {
        return axum::Json(serde_json::json!({
            "status": "error",
            "message": "Missing required fields: data_file, start, end"
        }));
    }

    let param_sets = match build_param_grid(strategy, mode, capital) {
        Ok(v) => v,
        Err(e) => {
            return axum::Json(serde_json::json!({"status":"error","message": e.to_string()}))
        }
    };

    let data_path = if data_file.starts_with('/') || data_file.starts_with("backtests/") {
        data_file.to_string()
    } else {
        format!("backtests/data/{data_file}")
    };
    let historical_data = match crate::data::loader::load_csv_data_in_range(&data_path, start, end)
    {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => {
            return axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("No candles in range {start} → {end} for {data_file}")
            }))
        }
        Err(e) => {
            return axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("Data load failed: {e}")
            }))
        }
    };
    let candle_count = historical_data.len();

    // Optional baseline (user's current params)
    let mut baseline_json = None;
    if !baseline_params.trim().is_empty() {
        if let Ok(base) =
            run_backtest_result(strategy, &baseline_params, data_file, capital, start, end).await
        {
            baseline_json = Some(base);
        }
    }

    #[derive(Clone)]
    struct Row {
        params: String,
        total_return_pct: f64,
        sharpe_ratio: f64,
        max_drawdown_pct: f64,
        total_trades: u64,
        win_rate_pct: f64,
        profit_factor: f64,
        score: f64,
    }

    let mut rows: Vec<Row> = Vec::with_capacity(param_sets.len());
    for params in &param_sets {
        let normalized = normalize_backtest_params(strategy, params, capital);
        let bt_strategy =
            match crate::strategy::create_strategy_with_params(strategy, Some(normalized.as_str()))
            {
                Ok(s) => s,
                Err(_) => continue,
            };
        let mut engine =
            crate::backtest::engine::BacktestEngine::new(capital, historical_data.clone());
        let results = match engine.run(bt_strategy).await {
            Ok(r) => r,
            Err(_) => continue,
        };
        let ret = results.total_return * 100.0;
        let dd = results.max_drawdown * 100.0;
        let sharpe = results.sharpe_ratio;
        let trades = results.total_trades as u64;
        rows.push(Row {
            params: normalized,
            total_return_pct: ret,
            sharpe_ratio: sharpe,
            max_drawdown_pct: dd,
            total_trades: trades,
            win_rate_pct: results.win_rate * 100.0,
            profit_factor: results.profit_factor,
            score: score_optimize_row(goal, ret, sharpe, dd, trades),
        });
    }

    rows.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let top_n = 15usize;
    let top: Vec<serde_json::Value> = rows
        .iter()
        .take(top_n)
        .enumerate()
        .map(|(i, r)| {
            serde_json::json!({
                "rank": i + 1,
                "params": r.params,
                "total_return_pct": r.total_return_pct,
                "sharpe_ratio": r.sharpe_ratio,
                "max_drawdown_pct": r.max_drawdown_pct,
                "total_trades": r.total_trades,
                "win_rate_pct": r.win_rate_pct,
                "profit_factor": r.profit_factor,
                "score": r.score,
            })
        })
        .collect();

    let best_params = rows.first().map(|r| r.params.clone());
    let best_detail = if let Some(ref p) = best_params {
        run_backtest_result(strategy, p, data_file, capital, start, end)
            .await
            .ok()
    } else {
        None
    };

    let profitable = rows.iter().filter(|r| r.total_return_pct > 0.0).count();
    let with_trades = rows.iter().filter(|r| r.total_trades > 0).count();

    axum::Json(serde_json::json!({
        "status": "ok",
        "mode": mode,
        "goal": goal,
        "strategy": strategy,
        "data_file": data_file,
        "start": start,
        "end": end,
        "candles": candle_count,
        "tested": rows.len(),
        "with_trades": with_trades,
        "profitable": profitable,
        "optimized_params": best_params,
        "baseline": baseline_json,
        "optimized": best_detail,
        "leaderboard": top,
        "message": format!(
            "Scanned {n} combos on {candles} candles; {profit} profitable, {wt} with trades. Best by {goal}.",
            n = rows.len(),
            candles = candle_count,
            profit = profitable,
            wt = with_trades,
            goal = goal
        ),
    }))
}

fn build_opencode_prompt(base: &serde_json::Value, params: &str, goal: &str) -> String {
    let goal_text = match goal {
        "return" => "Maximize total return",
        "drawdown" => "Minimize max drawdown",
        "balanced" => "Balance return and risk",
        _ => "Maximize Sharpe ratio",
    };
    format!(
        "Optimize this crypto grid strategy.\n\
Current params: {params}\n\
Return={ret:.2}% Sharpe={sharpe:.2} MaxDD={dd:.2}% ProfitFactor={pf:.2} Trades={trades}\n\
Goal: {goal_text}\n\
Allowed: grid_count 4-20, investment 3-80, deviation 0.005-0.03\n\
Reply with EXACTLY ONE LINE ONLY:\n\
PARAMS: grid_count=X,investment=Y,deviation=Z",
        ret = base["total_return_pct"].as_f64().unwrap_or(0.0),
        sharpe = base["sharpe_ratio"].as_f64().unwrap_or(0.0),
        dd = base["max_drawdown_pct"].as_f64().unwrap_or(0.0),
        pf = base["profit_factor"].as_f64().unwrap_or(0.0),
        trades = base["total_trades"].as_u64().unwrap_or(0),
    )
}

fn parse_suggested_params(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("PARAMS:") {
            return Some(rest.trim().to_string());
        }
    }
    let compact = text.replace(' ', "");
    let gc = compact
        .split("grid_count=")
        .nth(1)?
        .split([',', '\n'])
        .next()?;
    let inv = compact
        .split("investment=")
        .nth(1)?
        .split([',', '\n'])
        .next()?;
    let dev = compact
        .split("deviation=")
        .nth(1)?
        .split([',', '\n'])
        .next()?;
    Some(format!("grid_count={gc},investment={inv},deviation={dev}"))
}

async fn opencode_optimize_handler(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let strategy = body
        .get("strategy")
        .and_then(|s| s.as_str())
        .unwrap_or("grid");
    let params = body
        .get("params")
        .and_then(|s| s.as_str())
        .unwrap_or("grid_count=10,investment=8,deviation=0.012");
    let data_file = body.get("data_file").and_then(|s| s.as_str()).unwrap_or("");
    let capital = body
        .get("capital")
        .and_then(|c| c.as_f64())
        .unwrap_or(125.0);
    let start = body.get("start").and_then(|s| s.as_str()).unwrap_or("");
    let end = body.get("end").and_then(|s| s.as_str()).unwrap_or("");
    let goal = body
        .get("goal")
        .and_then(|s| s.as_str())
        .unwrap_or("sharpe");
    // No silent GLM5 default — caller must pass an explicit local model.
    let model = body
        .get("opencode_model")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(model) = model else {
        return axum::Json(serde_json::json!({
            "status": "error",
            "message": "opencode_model is required. Prefer the dashboard AI Lab provider/model fields for normal optimization; OpenCode is an optional local CLI path only."
        }));
    };

    let base = match run_backtest_result(strategy, params, data_file, capital, start, end).await {
        Ok(result) => result,
        Err(e) => {
            return axum::Json(serde_json::json!({"status": "error", "message": e.to_string()}))
        }
    };

    let prompt = build_opencode_prompt(&base, params, goal);
    let output = match timeout(
        Duration::from_secs(240),
        Command::new("opencode")
            .arg("run")
            .arg("--pure")
            .arg("-m")
            .arg(model)
            .arg("--dir")
            .arg(".")
            .arg(prompt)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            return axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("OpenCode invocation failed: {}", e)
            }))
        }
        Err(_) => {
            return axum::Json(serde_json::json!({
                "status": "error",
                "message": "OpenCode request timed out after 240 seconds"
            }))
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let combined = if stdout.trim().is_empty() {
        stderr.clone()
    } else {
        format!("{stdout}\n{stderr}")
    };
    let suggested = match parse_suggested_params(&combined) {
        Some(value) => value,
        None => {
            let preview = combined.chars().take(500).collect::<String>();
            return axum::Json(serde_json::json!({
                "status": "error",
                "message": format!("OpenCode did not return parsable parameters. Output: {}", preview)
            }));
        }
    };

    match run_backtest_result(strategy, &suggested, data_file, capital, start, end).await {
        Ok(optimized) => axum::Json(serde_json::json!({
            "status": "ok",
            "model": model,
            "base": base,
            "optimized": optimized,
            "optimized_params": suggested,
            "suggestion": combined,
        })),
        Err(e) => axum::Json(serde_json::json!({
            "status": "error",
            "message": format!("Optimized backtest failed: {}", e),
            "base": base,
            "optimized_params": suggested,
            "suggestion": combined,
        })),
    }
}

async fn ai_page_handler() -> Html<&'static str> {
    Html(include_str!("ui/ai.html"))
}

async fn ai_js_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("ui/ai.js"),
    )
}

async fn quant_agent_js_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("ui/quant_agent.js"),
    )
}

async fn quant_agent_protocol_js_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("ui/quant_agent_protocol.js"),
    )
}

// ── Trading Control Endpoints ──

async fn markets_get_handler(
    State(state): State<SharedDashboardState>,
) -> axum::Json<serde_json::Value> {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "active_markets": ds.active_markets,
        "available_markets": ds.available_markets.iter().map(|(id, sym)| {
            serde_json::json!({ "id": id, "symbol": sym, "active": ds.active_markets.contains(id) })
        }).collect::<Vec<_>>(),
        "trading_paused": ds.trading_paused,
        "universe_mode": ds.universe_mode,
    }))
}

async fn markets_update_handler(
    State(state): State<SharedDashboardState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let mut ds = state.write().await;
    if let Some(mode) = body.get("universe_mode").and_then(|v| v.as_str()) {
        let mode = mode.trim().to_ascii_lowercase();
        if mode == "auto" || mode == "all" || mode == "explicit" {
            ds.universe_mode = if mode == "all" {
                "auto".to_string()
            } else {
                mode
            };
            PersistentStrategyConfig::save(&ds);
            info!("🎯 Universe mode updated: {}", ds.universe_mode);
        }
    }
    if let Some(markets) = body.get("markets").and_then(|v| v.as_array()) {
        let new_markets: Vec<u32> = markets
            .iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect();
        info!("📊 Trading markets updated: {:?}", new_markets);
        ds.active_markets = new_markets.clone();
        axum::Json(serde_json::json!({
            "status": "ok",
            "message": format!(
                "Active markets updated to {:?}, universe={}",
                new_markets, ds.universe_mode
            ),
            "active_markets": new_markets,
            "universe_mode": ds.universe_mode,
        }))
    } else if body.get("universe_mode").is_some() {
        axum::Json(serde_json::json!({
            "status": "ok",
            "message": format!("Universe mode set to {}", ds.universe_mode),
            "active_markets": ds.active_markets,
            "universe_mode": ds.universe_mode,
        }))
    } else {
        axum::Json(serde_json::json!({
            "status": "error",
            "message": "Invalid request. Expected: {\"markets\": [0, 1]} or {\"universe_mode\":\"auto\"}"
        }))
    }
}

async fn trading_pause_handler(
    State(state): State<SharedDashboardState>,
) -> axum::Json<serde_json::Value> {
    let mut ds = state.write().await;
    ds.trading_paused = true;
    info!("⏸️ Trading PAUSED via dashboard");
    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Trading paused. No new orders will be placed.",
        "trading_paused": true,
    }))
}

async fn trading_resume_handler(
    State(state): State<SharedDashboardState>,
) -> axum::Json<serde_json::Value> {
    let mut ds = state.write().await;
    ds.trading_paused = false;
    info!("▶️ Trading RESUMED via dashboard");
    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Trading resumed. Orders will be placed normally.",
        "trading_paused": false,
    }))
}

async fn cancel_all_handler(
    State(state): State<SharedDashboardState>,
) -> axum::Json<serde_json::Value> {
    let mut ds = state.write().await;
    ds.cancel_all_requested = true;
    info!("🗑️ Cancel all orders requested via dashboard");
    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Cancel all orders requested. Will execute on next cycle.",
    }))
}

async fn risk_config_get_handler(
    State(state): State<SharedDashboardState>,
) -> axum::Json<serde_json::Value> {
    let ds = state.read().await;
    axum::Json(ds.risk_config.clone())
}

async fn risk_config_update_handler(
    State(state): State<SharedDashboardState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let mut ds = state.write().await;
    // Update leverage_limit if provided
    if let Some(v) = body.get("leverage_limit").and_then(|v| v.as_f64()) {
        ds.leverage_limit = v;
    }
    // Store the update request for main loop to pick up
    ds.risk_update_requested = Some(body.clone());
    // Update cached config
    let fields = [
        "max_drawdown_pct",
        "daily_loss_limit_pct",
        "max_leverage",
        "position_stop_loss_pct",
        "position_take_profit_pct",
        "leverage_limit",
    ];
    for field in &fields {
        if let Some(v) = body.get(*field) {
            ds.risk_config[field] = v.clone();
        }
    }
    info!("🔧 Risk config updated from dashboard: {}", body);
    PersistentRiskConfig::save(&ds);
    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Risk parameters updated",
        "config": ds.risk_config.clone(),
    }))
}
