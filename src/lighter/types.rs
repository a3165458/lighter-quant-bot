use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[allow(dead_code)]
const _: () = (); // types module: all types are API surface

// ===== 交易方向 =====
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

// ===== 订单类型 =====
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderType {
    Limit,
    Market,
    StopLimit,
    StopMarket,
}

// ===== 订单状态 =====
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

// ===== K线数据 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candlestick {
    pub timestamp: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub symbol: String,
}

// ===== 订单簿价格层级 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: f64,
    pub quantity: f64,
}

// ===== 订单簿快照 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct OrderBook {
    pub symbol: String,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub timestamp: DateTime<Utc>,
}

impl OrderBook {
    pub fn best_bid(&self) -> Option<f64> {
        self.bids.first().map(|l| l.price)
    }

    pub fn best_ask(&self) -> Option<f64> {
        self.asks.first().map(|l| l.price)
    }

    pub fn mid_price(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn spread(&self) -> Option<f64> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }
}

// ===== 成交记录 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trade {
    pub id: String,
    pub symbol: String,
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub timestamp: DateTime<Utc>,
}

// ===== 订单 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub price: f64,
    pub quantity: f64,
    pub filled_quantity: f64,
    pub status: OrderStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ===== 仓位 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub side: Side,
    pub size: f64,
    pub entry_price: f64,
    pub unrealized_pnl: f64,
    pub leverage: f64,
}

// ===== 账户余额 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub asset: String,
    pub free: f64,
    pub locked: f64,
}

// ===== 账户信息 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub balances: Vec<Balance>,
    pub positions: Vec<Position>,
    pub total_equity: f64,
}

// ===== WebSocket消息 =====
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum WsMessage {
    OrderBookUpdate(OrderBook),
    TradeUpdate(Trade),
    OrderUpdate(Order),
    PositionUpdate(Position),
    AccountUpdate(AccountInfo),
    Ping,
    Pong,
    Error(String),
}

// ===== 交易信号 =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeSignal {
    pub symbol: String,
    pub side: Side,
    pub price: f64,
    pub quantity: f64,
    pub order_type: OrderType,
    pub reason: String,
    pub timestamp: DateTime<Utc>,
}

// ===== 市场快照 =====
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct MarketSnapshot {
    pub order_books: std::collections::HashMap<String, OrderBook>,
    pub recent_trades: Vec<Trade>,
    pub candles: std::collections::HashMap<String, Vec<Candlestick>>,
}

// ===== 下单请求 =====
#[derive(Debug, Clone, Serialize)]
pub struct PlaceOrderRequest {
    pub symbol: String,
    pub side: Side,
    pub order_type: OrderType,
    pub price: f64,
    pub quantity: f64,
}

// ===== 下单响应 =====
#[derive(Debug, Clone, Deserialize)]
pub struct PlaceOrderResponse {
    pub order_id: String,
    pub status: String,
}
