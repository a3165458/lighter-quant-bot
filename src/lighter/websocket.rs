use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use super::types::*;

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
    >,
    Message
>;

type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
    >
>;

/// Lighter WebSocket client for real-time market data
pub struct LighterWebSocket {
    ws_url: String,
    sender: Arc<RwLock<Option<WsSink>>>,
    broadcast_tx: broadcast::Sender<WsMessage>,
    cancel_token: CancellationToken,
    /// Track subscribed market IDs for re-subscription after reconnect
    subscribed_markets: Arc<Mutex<Vec<u32>>>,
}

impl LighterWebSocket {
    pub fn new(ws_url: &str) -> Self {
        let (broadcast_tx, _) = broadcast::channel(4096);
        Self {
            ws_url: ws_url.to_string(),
            sender: Arc::new(RwLock::new(None)),
            broadcast_tx,
            cancel_token: CancellationToken::new(),
            subscribed_markets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Connect to the WebSocket endpoint
    pub async fn connect(&self) -> Result<()> {
        info!("Connecting WebSocket: {}", self.ws_url);

        let (ws_stream, _) = connect_async(&self.ws_url).await
            .map_err(|e| anyhow::anyhow!("WebSocket connection failed: {}", e))?;

        let (write, read) = ws_stream.split();

        {
            let mut sender = self.sender.write().await;
            *sender = Some(write);
        }

        // Spawn read task with auto-reconnect
        let broadcast_tx = self.broadcast_tx.clone();
        let ws_url = self.ws_url.clone();
        let sender_clone = self.sender.clone();
        let cancel = self.cancel_token.clone();
        let subs = self.subscribed_markets.clone();

        tokio::spawn(async move {
            let mut read = read;

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("WebSocket read task cancelled");
                        break;
                    }
                    msg_opt = read.next() => {
                        match msg_opt {
                            Some(Ok(Message::Text(text))) => {
                                match Self::parse_message(&text) {
                                    Ok(Some(ws_msg)) => {
                                        if broadcast_tx.send(ws_msg).is_err() {
                                            debug!("No active receivers");
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        warn!("Failed to parse WS message: {} - raw: {}", e, &text[..text.len().min(200)]);
                                    }
                                }
                            }
                            Some(Ok(Message::Ping(data))) => {
                                let mut sender = sender_clone.write().await;
                                if let Some(ref mut ws) = *sender {
                                    let _ = ws.send(Message::Pong(data)).await;
                                }
                            }
                            Some(Ok(Message::Pong(_))) => {
                                // Pong received — connection alive
                            }
                            Some(Ok(Message::Close(_))) => {
                                warn!("WebSocket closed by server, reconnecting...");
                                match Self::reconnect_and_resubscribe(&ws_url, &sender_clone, &subs).await {
                                    Ok(new_read) => { read = new_read; }
                                    Err(_) => break,
                                }
                            }
                            Some(Err(e)) => {
                                error!("WebSocket error: {}, reconnecting...", e);
                                match Self::reconnect_and_resubscribe(&ws_url, &sender_clone, &subs).await {
                                    Ok(new_read) => { read = new_read; }
                                    Err(_) => break,
                                }
                            }
                            None => {
                                warn!("WebSocket stream ended, reconnecting...");
                                match Self::reconnect_and_resubscribe(&ws_url, &sender_clone, &subs).await {
                                    Ok(new_read) => { read = new_read; }
                                    Err(_) => break,
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            info!("WebSocket read task ended");
        });

        // Keepalive: ping every 30 seconds to prevent server idle timeout
        let sender_clone = self.sender.clone();
        let cancel = self.cancel_token.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("Keepalive task cancelled");
                        break;
                    }
                    _ = interval.tick() => {
                        let mut sender = sender_clone.write().await;
                        if let Some(ref mut ws) = *sender {
                            if ws.send(Message::Ping(vec![0x42])).await.is_err() {
                                warn!("Keepalive ping failed, will retry next cycle");
                            }
                        }
                    }
                }
            }
        });

        info!("WebSocket connected successfully");
        Ok(())
    }

    /// Subscribe to market data for a symbol/market_id.
    pub async fn subscribe_market_data(&self, symbol: &str) -> Result<()> {
        let market_id = Self::resolve_market_id(symbol);

        Self::send_subscriptions(&self.sender, market_id).await?;

        // Track subscription for reconnect re-subscription
        {
            let mut subs = self.subscribed_markets.lock().await;
            if !subs.contains(&market_id) {
                subs.push(market_id);
            }
        }

        info!("Subscribed to market data: {} (market_id={})", symbol, market_id);
        Ok(())
    }

    /// Send subscription messages for a specific market
    async fn send_subscriptions(
        sender: &Arc<RwLock<Option<WsSink>>>,
        market_id: u32,
    ) -> Result<()> {
        let mut s = sender.write().await;
        if let Some(ref mut ws) = *s {
            for channel in &["order_book", "trade", "ticker"] {
                let sub = serde_json::json!({
                    "type": "subscribe",
                    "channel": format!("{}/{}", channel, market_id),
                });
                ws.send(Message::Text(sub.to_string())).await
                    .map_err(|e| anyhow::anyhow!("Subscription send failed: {}", e))?;
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!("WebSocket not connected"))
        }
    }

    /// Unsubscribe from market data
    #[allow(dead_code)]
    pub async fn unsubscribe(&self, symbol: &str) -> Result<()> {
        let market_id = Self::resolve_market_id(symbol);

        for channel in &["order_book", "trade", "ticker"] {
            let msg = serde_json::json!({
                "type": "unsubscribe",
                "channel": format!("{}/{}", channel, market_id),
            });
            self.send_message(&msg.to_string()).await?;
        }

        info!("Unsubscribed: {} (market_id={})", symbol, market_id);
        Ok(())
    }

    /// Send a raw text message
    async fn send_message(&self, msg: &str) -> Result<()> {
        let mut sender = self.sender.write().await;
        if let Some(ref mut ws) = *sender {
            ws.send(Message::Text(msg.to_string())).await
                .map_err(|e| anyhow::anyhow!("Failed to send WebSocket message: {}", e))?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("WebSocket not connected"))
        }
    }

    /// Get a broadcast receiver for WsMessage
    pub fn get_receiver(&self) -> broadcast::Receiver<WsMessage> {
        self.broadcast_tx.subscribe()
    }

    /// Resolve symbol string to market_id
    fn resolve_market_id(symbol: &str) -> u32 {
        // 优先按数字市场 ID 解析，其次查全局符号注册表
        if let Ok(id) = symbol.parse::<u32>() {
            return id;
        }
        match symbol.to_uppercase().as_str() {
            "ETHUSDC" | "ETH-PERP" | "ETHPERP" => 0,
            "BTCUSDC" | "BTC-PERP" | "BTCPERP" => 1,
            other => super::symbols::market_id_of(other).unwrap_or(0),
        }
    }

    /// Map market_id to symbol string
    fn market_id_to_symbol(market_id: u32) -> String {
        super::symbols::symbol_of(market_id)
    }

    /// Extract market_id from channel string like "order_book:0" or "trade:1"
    fn parse_channel_market_id(channel: &str) -> Option<u32> {
        channel.split(':').nth(1)?.parse::<u32>().ok()
    }

    /// Reconnect with exponential backoff + re-subscribe to all channels
    async fn reconnect_and_resubscribe(
        ws_url: &str,
        sender: &Arc<RwLock<Option<WsSink>>>,
        subscribed_markets: &Arc<Mutex<Vec<u32>>>,
    ) -> Result<WsStream> {
        let mut delay = 5u64;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            warn!("🔄 WebSocket reconnect attempt {} (wait {}s)...", attempt, delay);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

            match connect_async(ws_url).await {
                Ok((new_stream, _)) => {
                    let (new_write, new_read) = new_stream.split();
                    {
                        let mut s = sender.write().await;
                        *s = Some(new_write);
                    }

                    // Re-subscribe to all previously subscribed markets
                    let markets = subscribed_markets.lock().await.clone();
                    for market_id in &markets {
                        if let Err(e) = Self::send_subscriptions(sender, *market_id).await {
                            warn!("Re-subscription for market {} failed: {}", market_id, e);
                        }
                    }

                    info!("✅ WebSocket reconnected on attempt {} (re-subscribed {} markets)", attempt, markets.len());
                    return Ok(new_read);
                }
                Err(e) => {
                    error!("❌ WebSocket reconnect attempt {} failed: {}", attempt, e);
                    delay = (delay * 2).min(30);
                }
            }
        }
    }

    /// Parse a WebSocket message from Lighter.
    /// Returns Ok(None) for non-data messages (subscription confirmations, etc.)
    fn parse_message(text: &str) -> Result<Option<WsMessage>> {
        let value: serde_json::Value = serde_json::from_str(text)?;

        let channel = value.get("channel").and_then(|c| c.as_str()).unwrap_or("");
        let msg_type = value.get("type").and_then(|t| t.as_str()).unwrap_or("unknown");

        // Order book update: channel="order_book:0", type="update/order_book"
        if channel.starts_with("order_book:") && msg_type.contains("order_book") {
            let market_id = Self::parse_channel_market_id(channel).unwrap_or(0);
            let symbol = Self::market_id_to_symbol(market_id);

            let ob_data = &value["order_book"];

            let parse_levels = |key: &str| -> Vec<PriceLevel> {
                ob_data[key]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|lvl| {
                                let price: f64 = lvl["price"].as_str()?.parse().ok()?;
                                let qty: f64 = lvl["size"].as_str()?.parse().ok()?;
                                Some(PriceLevel { price, quantity: qty })
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            };

            let ob = OrderBook {
                symbol,
                market_id,
                bids: parse_levels("bids"),
                asks: parse_levels("asks"),
                timestamp: chrono::Utc::now(),
            };

            return Ok(Some(WsMessage::OrderBookUpdate(ob)));
        }

        // Trade update: channel="trade:0", type="update/trade"
        if channel.starts_with("trade:") && msg_type.contains("trade") {
            let market_id = Self::parse_channel_market_id(channel).unwrap_or(0);
            let symbol = Self::market_id_to_symbol(market_id);

            if let Some(trades_arr) = value["trades"].as_array() {
                // Send only the latest trade
                if let Some(t) = trades_arr.last() {
                    let price: f64 = t["price"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0.0);
                    let qty: f64 = t["size"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0.0);
                    let side_str = t["side"].as_str().unwrap_or("buy");
                    let side = if side_str == "ask" || side_str == "sell" {
                        Side::Sell
                    } else {
                        Side::Buy
                    };
                    let ts = t["timestamp"]
                        .as_str()
                        .and_then(|s| s.parse::<i64>().ok())
                        .or_else(|| t["timestamp"].as_i64())
                        .unwrap_or(0);
                    let dt = chrono::DateTime::from_timestamp(ts, 0)
                        .unwrap_or_else(chrono::Utc::now);

                    let trade = Trade {
                        id: t["trade_index"]
                            .as_str()
                            .unwrap_or("0")
                            .to_string(),
                        symbol,
                        market_id,
                        price,
                        quantity: qty,
                        side,
                        timestamp: dt,
                    };

                    return Ok(Some(WsMessage::TradeUpdate(trade)));
                }
            }
        }

        // Ticker updates for price tracking
        if channel.starts_with("ticker:") && msg_type.contains("update") {
            let market_id = Self::parse_channel_market_id(channel).unwrap_or(0);
            let symbol = Self::market_id_to_symbol(market_id);

            // Try multiple locations for price data
            let price: f64 = value.get("ticker")
                .and_then(|t| t["last_trade_price"].as_f64()
                    .or_else(|| t["last_trade_price"].as_str().and_then(|s| s.parse().ok())))
                .or_else(|| value["last_trade_price"].as_f64())
                .or_else(|| value["last_trade_price"].as_str().and_then(|s| s.parse().ok()))
                .unwrap_or(0.0);
            if price > 0.0 {
                let trade = Trade {
                    id: "ticker".to_string(),
                    symbol,
                    market_id,
                    price,
                    quantity: 0.0,
                    side: Side::Buy,
                    timestamp: chrono::Utc::now(),
                };
                return Ok(Some(WsMessage::TradeUpdate(trade)));
            }
        }

        // Subscription confirmation or unknown — ignore
        if msg_type.contains("subscribed") || msg_type == "connected" {
            return Ok(None);
        }

        if msg_type == "error" {
            let err_msg = value.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown WS error")
                .to_string();
            return Ok(Some(WsMessage::Error(err_msg)));
        }

        debug!("Unknown WebSocket message type: {} channel: {}", msg_type, channel);
        Ok(None)
    }
}
