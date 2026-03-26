use crate::lighter::types::Candlestick;

/// 数据回放器，用于按时间顺序回放历史K线数据
#[allow(dead_code)]
pub struct DataReplayer {
    data: Vec<Candlestick>,
    current_index: usize,
    speed_multiplier: f64,
}

#[allow(dead_code)]
impl DataReplayer {
    pub fn new(data: Vec<Candlestick>) -> Self {
        Self {
            data,
            current_index: 0,
            speed_multiplier: 1.0,
        }
    }

    /// 设置回放速度倍率
    pub fn set_speed(&mut self, multiplier: f64) {
        self.speed_multiplier = multiplier;
    }

    /// 获取下一根K线
    pub fn next(&mut self) -> Option<&Candlestick> {
        if self.current_index < self.data.len() {
            let candle = &self.data[self.current_index];
            self.current_index += 1;
            Some(candle)
        } else {
            None
        }
    }

    /// 重置回放位置
    pub fn reset(&mut self) {
        self.current_index = 0;
    }

    /// 获取总数据量
    pub fn total_candles(&self) -> usize {
        self.data.len()
    }

    /// 获取当前进度（百分比）
    pub fn progress(&self) -> f64 {
        if self.data.is_empty() {
            return 100.0;
        }
        (self.current_index as f64 / self.data.len() as f64) * 100.0
    }

    /// 跳转到指定位置
    pub fn seek(&mut self, index: usize) {
        self.current_index = index.min(self.data.len());
    }

    /// 获取指定范围的数据
    pub fn get_range(&self, start: usize, end: usize) -> &[Candlestick] {
        let end = end.min(self.data.len());
        let start = start.min(end);
        &self.data[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_candle(price: f64) -> Candlestick {
        Candlestick {
            timestamp: Utc::now(),
            open: price,
            high: price * 1.01,
            low: price * 0.99,
            close: price,
            volume: 100.0,
            symbol: "BTCUSDT".to_string(),
        }
    }

    #[test]
    fn test_replayer() {
        let data = vec![make_candle(100.0), make_candle(101.0), make_candle(102.0)];
        let mut replayer = DataReplayer::new(data);

        assert_eq!(replayer.total_candles(), 3);
        assert_eq!(replayer.progress(), 0.0);

        assert!(replayer.next().is_some());
        assert!(replayer.next().is_some());
        assert!(replayer.next().is_some());
        assert!(replayer.next().is_none());

        replayer.reset();
        assert_eq!(replayer.progress(), 0.0);
    }
}
