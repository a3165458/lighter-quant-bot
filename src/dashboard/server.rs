use anyhow::Result;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tracing::info;

/// 启动Dashboard Web服务
pub async fn start(host: &str, port: u16) -> Result<()> {
    let app = Router::new()
        .route("/", get(index_handler))
        .route("/app.js", get(js_handler))
        .route("/health", get(health_handler))
        .route("/ws", get(ws_handler))
        .route("/api/status", get(status_handler))
        .route("/api/positions", get(positions_handler))
        .route("/api/trades", get(trades_handler))
        .layer(CorsLayer::permissive());

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    info!("Dashboard 启动: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// 首页
async fn index_handler() -> Html<&'static str> {
    Html(include_str!("ui/index.html"))
}

/// JavaScript 文件
async fn js_handler() -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/javascript")],
        include_str!("ui/app.js"),
    )
}

/// 健康检查
async fn health_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "uptime": "running"
    }))
}

/// WebSocket处理
async fn ws_handler(ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(handle_ws_connection)
}

async fn handle_ws_connection(mut socket: WebSocket) {
    info!("新的WebSocket连接");

    // 发送欢迎消息
    let welcome = serde_json::json!({
        "type": "welcome",
        "message": "Connected to Lighter Bot Dashboard"
    });
    let _ = socket.send(Message::Text(welcome.to_string())).await;

    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    let msg_type = parsed.get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown");

                    let response = match msg_type {
                        "status" => serde_json::json!({
                            "type": "status",
                            "data": {
                                "running": true,
                                "uptime_seconds": 0,
                                "active_strategies": [],
                                "total_trades": 0
                            }
                        }),
                        "positions" => serde_json::json!({
                            "type": "positions",
                            "data": []
                        }),
                        "recent_trades" => serde_json::json!({
                            "type": "recent_trades",
                            "data": []
                        }),
                        "symbols" => serde_json::json!({
                            "type": "symbols",
                            "data": ["BTCUSDT", "ETHUSDT"]
                        }),
                        _ => serde_json::json!({
                            "type": "error",
                            "message": format!("Unknown command: {}", msg_type)
                        }),
                    };

                    let _ = socket.send(Message::Text(response.to_string())).await;
                }
            }
            Ok(Message::Close(_)) => {
                info!("WebSocket连接关闭");
                break;
            }
            Err(e) => {
                tracing::error!("WebSocket错误: {}", e);
                break;
            }
            _ => {}
        }
    }
}

/// 系统状态API
async fn status_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "running",
        "uptime_seconds": 0,
        "version": env!("CARGO_PKG_VERSION"),
        "active_strategies": [],
        "total_trades": 0,
        "total_pnl": 0.0
    }))
}

/// 持仓API
async fn positions_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "positions": []
    }))
}

/// 交易记录API
async fn trades_handler() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "trades": []
    }))
}
