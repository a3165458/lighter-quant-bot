use anyhow::Result;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{Html, IntoResponse},
    routing::{get, post},
    Router,
};
use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{info, warn};
use serde::{Serialize, Deserialize};

const PNL_STATE_FILE: &str = "data/pnl_state.json";

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
}

impl PersistentPnlData {
    pub fn load() -> Option<Self> {
        let data = std::fs::read_to_string(PNL_STATE_FILE).ok()?;
        match serde_json::from_str(&data) {
            Ok(state) => {
                info!("📂 Loaded PnL state from {}", PNL_STATE_FILE);
                Some(state)
            }
            Err(e) => {
                warn!("⚠️ Failed to parse PnL state file: {}", e);
                None
            }
        }
    }

    pub fn save(&self) {
        // Ensure data directory exists
        let _ = std::fs::create_dir_all("data");
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = std::fs::write(PNL_STATE_FILE, json) {
                    warn!("⚠️ Failed to save PnL state: {}", e);
                }
            }
            Err(e) => warn!("⚠️ Failed to serialize PnL state: {}", e),
        }
    }
}

/// Shared dashboard state
#[derive(Clone, Default)]
pub struct DashboardState {
    pub equity: f64,
    pub available_balance: f64,
    pub unrealized_pnl: f64,
    pub strategy_name: String,
    pub total_trades: u64,
    pub open_orders: u32,
    pub open_orders_list: Vec<serde_json::Value>,
    pub positions: Vec<serde_json::Value>,
    pub trade_history: Vec<serde_json::Value>,
    pub order_books: std::collections::HashMap<u32, serde_json::Value>,
    pub risk_status: Option<serde_json::Value>,
    // PnL tracking
    pub daily_realized_pnl: f64,
    pub total_realized_pnl: f64,
    pub initial_equity: f64,
    pub peak_equity: f64,
    pub equity_history: Vec<(i64, f64)>, // (unix_ts, equity) — for chart
    pub pnl_history: Vec<(i64, f64)>,    // (unix_ts, cumulative_pnl)
    // Strategy config (can be modified from dashboard)
    pub strategy_params: std::collections::HashMap<String, String>,
    pub strategy_config_changed: bool,
    // Per-day PnL tracking (persisted)
    pub daily_pnl_map: std::collections::HashMap<String, f64>,
    // Trading controls (runtime)
    pub active_markets: Vec<u32>,           // Markets currently being traded
    pub trading_paused: bool,               // Pause all trading signals
    pub cancel_all_requested: bool,         // Request to cancel all open orders
    pub available_markets: Vec<(u32, String)>, // All known markets: (id, symbol)
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
        };
        persistent.save();
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
        // Restore today's daily PnL
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        self.daily_realized_pnl = data.daily_pnl_map.get(&today).copied().unwrap_or(0.0);
        self.daily_pnl_map = data.daily_pnl_map.clone();
        info!("📂 Restored PnL: total={:.4}, daily={:.4}, peak={:.2}, trades={}",
            self.total_realized_pnl, self.daily_realized_pnl, self.peak_equity, self.trade_history.len());
    }
}

pub type SharedDashboardState = Arc<RwLock<DashboardState>>;

/// Start Dashboard Web server
pub async fn start(host: &str, port: u16) -> Result<()> {
    let state: SharedDashboardState = Arc::new(RwLock::new(DashboardState::default()));
    start_with_state(host, port, state).await
}

pub async fn start_with_state(host: &str, port: u16, state: SharedDashboardState) -> Result<()> {
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/ai", get(ai_page_handler))
        .route("/ai.js", get(ai_js_handler))
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .route("/api/status", get(status_handler))
        .route("/api/positions", get(positions_handler))
        .route("/api/trades", get(trades_handler))
        .route("/api/pnl", get(pnl_handler))
        .route("/api/strategy", get(strategy_get_handler))
        .route("/api/strategy", post(strategy_update_handler))
        .route("/api/backtest", post(backtest_handler))
        .route("/api/trading/markets", get(markets_get_handler))
        .route("/api/trading/markets", post(markets_update_handler))
        .route("/api/trading/pause", post(trading_pause_handler))
        .route("/api/trading/resume", post(trading_resume_handler))
        .route("/api/trading/cancel-all", post(cancel_all_handler))
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
                    "daily_realized_pnl": ds.daily_realized_pnl,
                    "total_realized_pnl": ds.total_realized_pnl,
                    "initial_equity": ds.initial_equity,
                    "peak_equity": ds.peak_equity,
                    "total_return_pct": if ds.initial_equity > 0.0 {
                        (ds.equity - ds.initial_equity) / ds.initial_equity * 100.0
                    } else { 0.0 },
                    "trading_paused": ds.trading_paused,
                    "active_markets": ds.active_markets,
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
            let orders_msg = serde_json::json!({
                "type": "open_orders",
                "data": ds.open_orders_list
            });
            // Collect orderbook snapshots for all markets
            let orderbook_msgs: Vec<_> = ds.order_books.values().map(|ob| {
                serde_json::json!({ "type": "orderbook", "data": ob })
            }).collect();
            drop(ds);

            if ws_sender.send(Message::Text(status_msg.to_string())).await.is_err() {
                break;
            }
            let _ = ws_sender.send(Message::Text(positions_msg.to_string())).await;
            let _ = ws_sender.send(Message::Text(risk_msg.to_string())).await;
            let _ = ws_sender.send(Message::Text(trades_msg.to_string())).await;
            let _ = ws_sender.send(Message::Text(orders_msg.to_string())).await;
            for ob_msg in orderbook_msgs {
                let _ = ws_sender.send(Message::Text(ob_msg.to_string())).await;
            }
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
    let account_idx = std::env::var("LIGHTER_ACCOUNT_INDEX").unwrap_or_default();
    axum::Json(serde_json::json!({
        "status": "running",
        "version": env!("CARGO_PKG_VERSION"),
        "strategy": ds.strategy_name,
        "total_trades": ds.total_trades,
        "equity": ds.equity,
        "total_pnl": ds.unrealized_pnl,
        "daily_realized_pnl": ds.daily_realized_pnl,
        "total_realized_pnl": ds.total_realized_pnl,
        "account_index": account_idx,
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

async fn pnl_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "daily_realized_pnl": ds.daily_realized_pnl,
        "total_realized_pnl": ds.total_realized_pnl,
        "unrealized_pnl": ds.unrealized_pnl,
        "equity": ds.equity,
        "initial_equity": ds.initial_equity,
        "peak_equity": ds.peak_equity,
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
        "trades": ds.trade_history.iter().rev().take(50).collect::<Vec<_>>(),
    }))
}

async fn strategy_get_handler(State(state): State<SharedDashboardState>) -> impl IntoResponse {
    let ds = state.read().await;
    axum::Json(serde_json::json!({
        "strategy": ds.strategy_name,
        "params": ds.strategy_params,
    }))
}

async fn strategy_update_handler(
    State(state): State<SharedDashboardState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let mut ds = state.write().await;
    if let Some(params) = body.get("params").and_then(|p| p.as_object()) {
        for (k, v) in params {
            ds.strategy_params.insert(
                k.clone(),
                v.as_str().map(|s| s.to_string())
                    .or_else(|| v.as_f64().map(|n| n.to_string()))
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
                    .unwrap_or_default(),
            );
        }
        ds.strategy_config_changed = true;
        info!("Strategy params updated from dashboard: {:?}", ds.strategy_params);
    }
    axum::Json(serde_json::json!({
        "status": "ok",
        "message": "Strategy params updated. Will apply on next grid reset cycle.",
        "params": ds.strategy_params,
    }))
}

async fn backtest_handler(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let strategy = body.get("strategy").and_then(|s| s.as_str()).unwrap_or("grid");
    let params = body.get("params").and_then(|s| s.as_str()).unwrap_or("");
    let data_file = body.get("data_file").and_then(|s| s.as_str()).unwrap_or("");
    let capital = body.get("capital").and_then(|c| c.as_f64()).unwrap_or(125.0);
    let start = body.get("start").and_then(|s| s.as_str()).unwrap_or("");
    let end = body.get("end").and_then(|s| s.as_str()).unwrap_or("");

    // Validate inputs
    if data_file.is_empty() || start.is_empty() || end.is_empty() {
        return axum::Json(serde_json::json!({
            "status": "error",
            "message": "Missing required fields: data_file, start, end"
        }));
    }

    // Run backtest in-process
    let data_path = if data_file.starts_with('/') || data_file.starts_with("backtests/") {
        data_file.to_string()
    } else {
        format!("backtests/data/{}", data_file)
    };

    match crate::data::loader::load_csv_data_in_range(&data_path, start, end) {
        Ok(historical_data) => {
            let candle_count = historical_data.len();
            match crate::strategy::create_strategy_with_params(strategy, if params.is_empty() { None } else { Some(params) }) {
                Ok(bt_strategy) => {
                    let mut engine = crate::backtest::engine::BacktestEngine::new(capital, historical_data);
                    match engine.run(bt_strategy).await {
                        Ok(results) => {
                            axum::Json(serde_json::json!({
                                "status": "ok",
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
                        Err(e) => axum::Json(serde_json::json!({"status": "error", "message": format!("Backtest failed: {}", e)})),
                    }
                }
                Err(e) => axum::Json(serde_json::json!({"status": "error", "message": format!("Invalid strategy: {}", e)})),
            }
        }
        Err(e) => axum::Json(serde_json::json!({"status": "error", "message": format!("Data load failed: {}", e)})),
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
    }))
}

async fn markets_update_handler(
    State(state): State<SharedDashboardState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let mut ds = state.write().await;
    if let Some(markets) = body.get("markets").and_then(|v| v.as_array()) {
        let new_markets: Vec<u32> = markets.iter()
            .filter_map(|v| v.as_u64().map(|n| n as u32))
            .collect();
        info!("📊 Trading markets updated: {:?}", new_markets);
        ds.active_markets = new_markets.clone();
        axum::Json(serde_json::json!({
            "status": "ok",
            "message": format!("Active markets updated to {:?}", new_markets),
            "active_markets": new_markets,
        }))
    } else {
        axum::Json(serde_json::json!({
            "status": "error",
            "message": "Invalid request. Expected: {\"markets\": [0, 1]}"
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
