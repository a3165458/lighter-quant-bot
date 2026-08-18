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
        self.order_books
            .insert(order_book.symbol.clone(), order_book);
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
        let candles = self
            .candles
            .entry(candle.symbol.clone())
            .or_insert_with(|| Vec::with_capacity(256));
        candles.push(candle);

        if candles.len() > self.max_candles {
            let keep_from = candles.len() - (self.max_candles * 4 / 5);
            candles.drain(..keep_from);
        }
    }

    /// 获取市场快照（仅克隆最近数据以减少开销）
    ///
    /// 订单簿在写入时先做价差合理性过滤：增量盘口更新的某一瞬间，买盘顶部可能还没补上，
    /// best_bid 会掉到很深的一档，中间价随之被拉歪（实测出现过 (63193+60040)/2 = 61616，
    /// 偏离 2.5% —— 见 main.rs 主循环对 dashboard last_prices 的过滤注释）。如果直接把
    /// 这种脏盘口原样塞进策略快照，趋势策略会拿错误的 mid 计算盈亏并触发虚假止损
    /// （2026-08-08 实盘事故：mid 被拉歪到 $58,554 vs 真实 ~$65,016，止损单打到交易所
    /// 被 "accidental price" 拦截）。过滤阈值与 main.rs 展示路径一致：价差 ≥0.5% 的
    /// 盘口视为残帧，直接剔除，策略与展示两侧都得不到污染数据。
    pub fn get_snapshot(&self) -> MarketSnapshot {
        let recent_candles: HashMap<String, Vec<Candlestick>> = self
            .candles
            .iter()
            .map(|(k, v)| {
                let start = v.len().saturating_sub(100);
                (k.clone(), v[start..].to_vec())
            })
            .collect();

        let recent_trades_start = self.recent_trades.len().saturating_sub(200);

        let order_books: HashMap<String, OrderBook> = self
            .order_books
            .iter()
            .filter(|(_, ob)| {
                let (Some(bid), Some(ask), Some(mid)) =
                    (ob.best_bid(), ob.best_ask(), ob.mid_price())
                else {
                    return false;
                };
                mid > 0.0 && bid > 0.0 && ask > bid && (ask - bid) / mid < 0.005
            })
            .map(|(symbol, ob)| (symbol.clone(), ob.clone()))
            .collect();

        MarketSnapshot {
            order_books,
            recent_trades: self.recent_trades[recent_trades_start..].to_vec(),
            candles: recent_candles,
            positions: std::collections::HashMap::new(),
            position_entry_prices: std::collections::HashMap::new(),
            positions_authoritative: false,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn book(symbol: &str, bid: f64, ask: f64) -> OrderBook {
        OrderBook {
            symbol: symbol.to_string(),
            market_id: 1,
            bids: vec![PriceLevel {
                price: bid,
                quantity: 1.0,
            }],
            asks: vec![PriceLevel {
                price: ask,
                quantity: 1.0,
            }],
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn get_snapshot_filters_polluted_order_books() {
        let mut store = MarketDataStore::new();
        // 健康盘口：价差 ~0.1% < 0.5%，应保留
        store.update_order_book(book("BTC", 65000.0, 65065.0));
        // 污染盘口：价差 ~9%（best_bid 塌到深档），复刻 2026-08-08 事故形态，应剔除
        store.update_order_book(book("ETH", 58554.0, 64000.0));
        // 残帧：只有 ask 没有 bid，应剔除
        let mut empty_bids = book("SOL", 1.0, 2.0);
        empty_bids.bids.clear();
        store.update_order_book(empty_bids);

        let snap = store.get_snapshot();
        assert!(snap.order_books.contains_key("BTC"), "healthy book kept");
        assert!(
            !snap.order_books.contains_key("ETH"),
            "wide-spread book dropped"
        );
        assert!(
            !snap.order_books.contains_key("SOL"),
            "bid-less book dropped"
        );
    }

    #[test]
    fn get_snapshot_keeps_clean_books_on_mixed_store() {
        let mut store = MarketDataStore::new();
        store.update_order_book(book("BTC", 65000.0, 65032.5));
        store.update_order_book(book("ETH", 3000.0, 3002.0));
        let snap = store.get_snapshot();
        assert!(snap.order_books.contains_key("BTC"));
        assert!(snap.order_books.contains_key("ETH"));
    }
}
