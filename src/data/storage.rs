use std::collections::HashMap;

use crate::lighter::types::*;

/// 市场数据存储
pub struct MarketDataStore {
    order_books: HashMap<String, OrderBook>,
    recent_trades: Vec<Trade>,
    candles: HashMap<String, Vec<Candlestick>>,
    max_trades: usize,
    #[allow(dead_code)]
    max_candles: usize,
}

impl MarketDataStore {
    pub fn new() -> Self {
        Self {
            order_books: HashMap::with_capacity(8),
            recent_trades: Vec::with_capacity(1024),
            candles: HashMap::with_capacity(8),
            max_trades: 10000,
            max_candles: 5000,
        }
    }

    /// 更新订单簿
    pub fn update_order_book(&mut self, order_book: OrderBook) {
        self.order_books.insert(order_book.symbol.clone(), order_book);
    }

    /// 添加成交记录
    pub fn add_trade(&mut self, trade: Trade) {
        self.recent_trades.push(trade);

        // 限制存储大小 — 一次性裁剪到 80% 容量以减少频繁 drain
        if self.recent_trades.len() > self.max_trades {
            let keep_from = self.recent_trades.len() - (self.max_trades * 4 / 5);
            self.recent_trades.drain(..keep_from);
        }
    }

    /// 添加K线数据
    #[allow(dead_code)]
    pub fn add_candle(&mut self, candle: Candlestick) {
        let candles = self.candles.entry(candle.symbol.clone()).or_insert_with(|| Vec::with_capacity(256));
        candles.push(candle);

        if candles.len() > self.max_candles {
            let keep_from = candles.len() - (self.max_candles * 4 / 5);
            candles.drain(..keep_from);
        }
    }

    /// 获取市场快照（仅克隆最近数据以减少开销）
    pub fn get_snapshot(&self) -> MarketSnapshot {
        let recent_candles: HashMap<String, Vec<Candlestick>> = self.candles.iter()
            .map(|(k, v)| {
                let start = v.len().saturating_sub(100);
                (k.clone(), v[start..].to_vec())
            })
            .collect();

        let recent_trades_start = self.recent_trades.len().saturating_sub(200);

        MarketSnapshot {
            order_books: self.order_books.clone(),
            recent_trades: self.recent_trades[recent_trades_start..].to_vec(),
            candles: recent_candles,
        }
    }

    /// 获取指定交易对的订单簿
    #[allow(dead_code)]
    pub fn get_order_book(&self, symbol: &str) -> Option<&OrderBook> {
        self.order_books.get(symbol)
    }

    /// 获取最近N条交易记录
    #[allow(dead_code)]
    pub fn get_recent_trades(&self, limit: usize) -> &[Trade] {
        let start = self.recent_trades.len().saturating_sub(limit);
        &self.recent_trades[start..]
    }

    /// 获取指定交易对的K线数据
    #[allow(dead_code)]
    pub fn get_candles(&self, symbol: &str) -> Option<&Vec<Candlestick>> {
        self.candles.get(symbol)
    }

    /// 清空所有数据
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.order_books.clear();
        self.recent_trades.clear();
        self.candles.clear();
    }
}

impl Default for MarketDataStore {
    fn default() -> Self {
        Self::new()
    }
}
