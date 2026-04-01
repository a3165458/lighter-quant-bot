use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

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
    StopLoss,
    StopLossLimit,
    TakeProfit,
    TakeProfitLimit,
    // Keep old variants as aliases
    #[serde(alias = "StopLimit")]
    StopLimit,
    #[serde(alias = "StopMarket")]
    StopMarket,
}

impl OrderType {
    /// Convert to Lighter protocol integer
    #[allow(dead_code)]
    pub fn to_lighter_int(&self) -> i32 {
        match self {
            OrderType::Limit => 0,
            OrderType::Market => 1,
            OrderType::StopLoss | OrderType::StopMarket => 2,
            OrderType::StopLossLimit | OrderType::StopLimit => 3,
            OrderType::TakeProfit => 4,
            OrderType::TakeProfitLimit => 5,
        }
    }
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

// ===== Market Info =====
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    pub market_id: u32,
    pub symbol: String,
    pub size_decimals: u32,
    pub price_decimals: u32,
    pub min_base_amount: f64,
    pub min_quote_amount: f64,
    pub last_trade_price: f64,
}

impl MarketInfo {
    /// Convert a floating-point size to integer base amount (e.g. 1.0 ETH -> 10000)
    #[allow(dead_code)]
    pub fn size_to_base_amount(&self, size: f64) -> i64 {
        let factor = 10_f64.powi(self.size_decimals as i32);
        (size * factor).round() as i64
    }

    /// Convert a floating-point price to integer price (e.g. $2070.00 -> 207000)
    #[allow(dead_code)]
    pub fn price_to_int(&self, price: f64) -> i32 {
        let factor = 10_f64.powi(self.price_decimals as i32);
        (price * factor).round() as i32
    }
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
    pub market_id: u32,
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
    pub market_id: u32,
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
    pub market_id: u32,
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
    pub order_books: HashMap<String, OrderBook>,
    pub recent_trades: Vec<Trade>,
    pub candles: HashMap<String, Vec<Candlestick>>,
}

// ===== 下单请求 =====
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
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

// ===== Trading State =====
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TradingState {
    pub market_infos: HashMap<u32, MarketInfo>,
    pub order_books: HashMap<u32, OrderBook>,
    pub recent_trades: HashMap<u32, Vec<Trade>>,
    pub candles: HashMap<u32, Vec<Candlestick>>,
    pub positions: Vec<Position>,
    pub account_info: Option<AccountInfo>,
    pub trade_history: Vec<Trade>,
    pub start_time: DateTime<Utc>,
}

impl Default for TradingState {
    fn default() -> Self {
        Self {
            market_infos: HashMap::new(),
            order_books: HashMap::new(),
            recent_trades: HashMap::new(),
            candles: HashMap::new(),
            positions: Vec::new(),
            account_info: None,
            trade_history: Vec::new(),
            start_time: Utc::now(),
        }
    }
}
