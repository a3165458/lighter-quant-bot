use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::types::*;

/// Lighter交易所WebSocket客户端
pub struct LighterWebSocket {
    ws_url: String,
    sender: Arc<RwLock<Option<futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
        >,
        Message
    >>>>,
    broadcast_tx: broadcast::Sender<WsMessage>,
    cancel_token: CancellationToken,
}

impl LighterWebSocket {
    pub fn new(ws_url: &str) -> Self {
        let (broadcast_tx, _) = broadcast::channel(4096);
        Self {
            ws_url: ws_url.to_string(),
            sender: Arc::new(RwLock::new(None)),
            broadcast_tx,
            cancel_token: CancellationToken::new(),
        }
    }

    /// 连接WebSocket
    pub async fn connect(&self) -> Result<()> {
        info!("连接 WebSocket: {}", self.ws_url);

        let (ws_stream, _) = connect_async(&self.ws_url).await
            .map_err(|e| anyhow::anyhow!("WebSocket连接失败: {}", e))?;

        let (write, read) = ws_stream.split();

        // 保存写端
        {
            let mut sender = self.sender.write().await;
            *sender = Some(write);
        }

        // 启动读取任务（带自动重连）
        let broadcast_tx = self.broadcast_tx.clone();
        let ws_url = self.ws_url.clone();
        let sender_clone = self.sender.clone();
        let cancel = self.cancel_token.clone();

        tokio::spawn(async move {
            let mut read = read;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("WebSocket读取任务被取消");
                        break;
                    }
                    msg_opt = read.next() => {
                        match msg_opt {
                            Some(Ok(Message::Text(text))) => {
                                match Self::parse_message(&text) {
                                    Ok(ws_msg) => {
                                        if broadcast_tx.send(ws_msg).is_err() {
                                            debug!("没有活跃的接收者");
                                        }
                                    }
                                    Err(e) => {
                                        warn!("解析WebSocket消息失败: {}", e);
                                    }
                                }
                            }
                            Some(Ok(Message::Ping(data))) => {
                                let mut sender = sender_clone.write().await;
                                if let Some(ref mut ws) = *sender {
                                    let _ = ws.send(Message::Pong(data)).await;
                                }
                            }
                            Some(Ok(Message::Close(_))) => {
                                warn!("WebSocket连接关闭: {}，5秒后重连...", ws_url);
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                // 尝试重连
                                match connect_async(&ws_url).await {
                                    Ok((new_stream, _)) => {
                                        let (new_write, new_read) = new_stream.split();
                                        {
                                            let mut sender = sender_clone.write().await;
                                            *sender = Some(new_write);
                                        }
                                        read = new_read;
                                        info!("WebSocket重连成功");
                                    }
                                    Err(e) => {
                                        error!("WebSocket重连失败: {}", e);
                                        break;
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                error!("WebSocket错误: {}，5秒后重连...", e);
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                match connect_async(&ws_url).await {
                                    Ok((new_stream, _)) => {
                                        let (new_write, new_read) = new_stream.split();
                                        {
                                            let mut sender = sender_clone.write().await;
                                            *sender = Some(new_write);
                                        }
                                        read = new_read;
                                        info!("WebSocket重连成功");
                                    }
                                    Err(e) => {
                                        error!("WebSocket重连失败: {}", e);
                                        break;
                                    }
                                }
                            }
                            None => {
                                warn!("WebSocket流结束");
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }

            info!("WebSocket读取任务结束");
        });

        // 启动心跳（带取消令牌）
        let sender_clone = self.sender.clone();
        let cancel = self.cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("心跳任务被取消");
                        break;
                    }
                    _ = interval.tick() => {
                        let mut sender = sender_clone.write().await;
                        if let Some(ref mut ws) = *sender {
                            if ws.send(Message::Ping(vec![])).await.is_err() {
                                warn!("心跳发送失败");
                                break;
                            }
                        }
                    }
                }
            }
        });

        info!("WebSocket连接成功");
        Ok(())
    }

    /// 订阅市场数据
    pub async fn subscribe_market_data(&self, symbol: &str) -> Result<()> {
        let subscribe_msg = serde_json::json!({
            "type": "subscribe",
            "channels": ["orderbook", "trades"],
            "symbol": symbol
        });

        self.send_message(&subscribe_msg.to_string()).await?;
        info!("已订阅市场数据: {}", symbol);
        Ok(())
    }

    /// 取消订阅
    #[allow(dead_code)]
    pub async fn unsubscribe(&self, symbol: &str) -> Result<()> {
        let msg = serde_json::json!({
            "type": "unsubscribe",
            "symbol": symbol
        });

        self.send_message(&msg.to_string()).await?;
        info!("已取消订阅: {}", symbol);
        Ok(())
    }

    /// 发送消息
    async fn send_message(&self, msg: &str) -> Result<()> {
        let mut sender = self.sender.write().await;
        if let Some(ref mut ws) = *sender {
            ws.send(Message::Text(msg.to_string())).await
                .map_err(|e| anyhow::anyhow!("发送WebSocket消息失败: {}", e))?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("WebSocket未连接"))
        }
    }

    /// 获取消息接收器
    pub fn get_receiver(&self) -> broadcast::Receiver<WsMessage> {
        self.broadcast_tx.subscribe()
    }

    /// 解析WebSocket消息
    fn parse_message(text: &str) -> Result<WsMessage> {
        let value: serde_json::Value = serde_json::from_str(text)?;

        let msg_type = value.get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("unknown");

        match msg_type {
            "orderbook" | "orderbook_update" => {
                let ob: OrderBook = serde_json::from_value(
                    value.get("data").cloned().unwrap_or(value.clone())
                )?;
                Ok(WsMessage::OrderBookUpdate(ob))
            }
            "trade" | "trade_update" => {
                let trade: Trade = serde_json::from_value(
                    value.get("data").cloned().unwrap_or(value.clone())
                )?;
                Ok(WsMessage::TradeUpdate(trade))
            }
            "order" | "order_update" => {
                let order: Order = serde_json::from_value(
                    value.get("data").cloned().unwrap_or(value.clone())
                )?;
                Ok(WsMessage::OrderUpdate(order))
            }
            "position" | "position_update" => {
                let pos: Position = serde_json::from_value(
                    value.get("data").cloned().unwrap_or(value.clone())
                )?;
                Ok(WsMessage::PositionUpdate(pos))
            }
            "ping" => Ok(WsMessage::Ping),
            "pong" => Ok(WsMessage::Pong),
            "error" => {
                let msg = value.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Unknown error")
                    .to_string();
                Ok(WsMessage::Error(msg))
            }
            _ => {
                debug!("未知WebSocket消息类型: {}", msg_type);
                Ok(WsMessage::Error(format!("Unknown message type: {}", msg_type)))
            }
        }
    }
}
