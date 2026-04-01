use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc, Duration};
use std::fs;
use std::io::BufRead;

use crate::lighter::types::Candlestick;

/// 从CSV文件加载历史数据
pub fn load_csv_data(path: &str) -> Result<Vec<Candlestick>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取文件失败: {}", path))?;

    let mut candles = Vec::new();
    let mut lines = content.as_bytes().lines();

    // 跳过表头
    let _header = lines.next();

    for line_result in lines {
        let line = line_result?;
        let fields: Vec<&str> = line.split(',').collect();

        if fields.len() < 6 {
            continue;
        }

        let timestamp = parse_timestamp(fields[0].trim())?;
        let open: f64 = fields[1].trim().parse().context("解析open失败")?;
        let high: f64 = fields[2].trim().parse().context("解析high失败")?;
        let low: f64 = fields[3].trim().parse().context("解析low失败")?;
        let close: f64 = fields[4].trim().parse().context("解析close失败")?;
        let volume: f64 = fields[5].trim().parse().context("解析volume失败")?;
        let symbol = if fields.len() > 6 {
            fields[6].trim().to_string()
        } else {
            "UNKNOWN".to_string()
        };

        candles.push(Candlestick {
            timestamp,
            open,
            high,
            low,
            close,
            volume,
            symbol,
        });
    }

    candles.sort_by_key(|c| c.timestamp);
    Ok(candles)
}

#[derive(Debug, Clone, Copy)]
pub enum RangeEnd {
    Inclusive(DateTime<Utc>),
    Exclusive(DateTime<Utc>),
}

pub fn parse_range_start(s: &str) -> Result<DateTime<Utc>> {
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(DateTime::from_naive_utc_and_offset(
            date.and_hms_opt(0, 0, 0).unwrap(),
            Utc,
        ));
    }

    parse_timestamp(s)
}

pub fn parse_range_end(s: &str) -> Result<RangeEnd> {
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(RangeEnd::Exclusive(DateTime::from_naive_utc_and_offset(
            (date + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap(),
            Utc,
        )));
    }

    Ok(RangeEnd::Inclusive(parse_timestamp(s)?))
}

pub fn load_csv_data_in_range(path: &str, start: &str, end: &str) -> Result<Vec<Candlestick>> {
    let candles = load_csv_data(path)?;
    let start_dt = parse_range_start(start)?;
    let end_dt = parse_range_end(end)?;

    let filtered: Vec<Candlestick> = candles
        .into_iter()
        .filter(|c| {
            c.timestamp >= start_dt
                && match end_dt {
                    RangeEnd::Inclusive(end) => c.timestamp <= end,
                    RangeEnd::Exclusive(end) => c.timestamp < end,
                }
        })
        .collect();

    if filtered.is_empty() {
        anyhow::bail!("指定日期范围内没有K线数据: {} -> {}", start, end);
    }

    Ok(filtered)
}

pub fn write_csv_data(path: &str, candles: &[Candlestick]) -> Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }

    let mut content = String::from("timestamp,open,high,low,close,volume,symbol\n");
    for candle in candles {
        content.push_str(&format!(
            "{},{:.6},{:.6},{:.6},{:.6},{:.6},{}\n",
            candle.timestamp.to_rfc3339(),
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume,
            candle.symbol
        ));
    }

    fs::write(path, content)?;
    Ok(())
}

/// 解析时间戳（支持多种格式）
fn parse_timestamp(s: &str) -> Result<DateTime<Utc>> {
    // 尝试 ISO 8601 格式
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // 尝试 "YYYY-MM-DD HH:MM:SS" 格式
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    // 尝试 "YYYY-MM-DDTHH:MM:SS" 格式
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(DateTime::from_naive_utc_and_offset(dt, Utc));
    }

    if let Ok(ts) = s.parse::<i64>() {
        let abs_ts = ts.unsigned_abs();
        let parsed = if abs_ts >= 1_000_000_000_000 {
            DateTime::from_timestamp(ts / 1000, ((ts % 1000) * 1_000_000) as u32)
        } else {
            DateTime::from_timestamp(ts, 0)
        };

        if let Some(dt) = parsed {
            return Ok(dt);
        }
    }

    anyhow::bail!("无法解析时间戳: {}", s)
}

/// 生成合成的测试数据
pub fn generate_synthetic_data(symbol: &str, days: u32) -> Result<()> {
    use rand::Rng;

    let mut rng = rand::thread_rng();

    // Realistic starting prices and volatility per symbol
    let (mut price, hourly_vol, vol_of_vol) = match symbol.to_uppercase().as_str() {
        "BTC" | "BTCUSDT" => (66500.0_f64, 0.003, 0.3),   // ~0.3% hourly vol
        "ETH" | "ETHUSDT" => (2020.0_f64, 0.004, 0.35),    // ~0.4% hourly vol
        _ => (1000.0_f64, 0.005, 0.3),
    };

    let mut candles = Vec::new();
    let start = Utc::now() - Duration::days(days as i64);
    let total_hours = days * 24;

    // Generate with mean-reversion + momentum regime switching
    let mut trend: f64 = 0.0;
    let mut vol_multiplier: f64 = 1.0;

    for i in 0..total_hours {
        let timestamp = start + Duration::hours(i as i64);

        // Regime switching: occasionally shift trend direction
        if rng.gen_range(0..100) < 3 {
            trend = rng.gen_range(-0.001..0.001);
        }
        // Mean-revert trend slowly
        trend *= 0.995;

        // Stochastic volatility
        vol_multiplier *= 1.0 + rng.gen_range(-vol_of_vol..vol_of_vol) * 0.1;
        vol_multiplier = vol_multiplier.clamp(0.3, 3.0);

        let effective_vol = hourly_vol * vol_multiplier;
        let change = trend + rng.gen_range(-effective_vol..effective_vol);
        let open = price;
        price *= 1.0 + change;

        let close = price;
        let high_ext = rng.gen_range(0.0..effective_vol * 0.5);
        let low_ext = rng.gen_range(0.0..effective_vol * 0.5);
        let high = f64::max(open, close) * (1.0 + high_ext);
        let low = f64::min(open, close) * (1.0 - low_ext);
        let volume: f64 = rng.gen_range(50.0..500.0) * vol_multiplier;

        candles.push(format!(
            "{},{:.2},{:.2},{:.2},{:.2},{:.4},{}",
            timestamp.to_rfc3339(),
            open, high, low, close, volume,
            symbol.to_uppercase()
        ));
    }

    let dir = "backtests/data";
    fs::create_dir_all(dir)?;

    let filename = format!("{}/{}-synthetic-{}d-1h.csv", dir, symbol.to_uppercase(), days);
    let mut content = String::from("timestamp,open,high,low,close,volume,symbol\n");
    for line in &candles {
        content.push_str(line);
        content.push('\n');
    }

    fs::write(&filename, content)?;
    tracing::info!("生成合成数据: {} ({} 条记录, {}天)", filename, candles.len(), days);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_parse_timestamp_rfc3339() {
        let ts = parse_timestamp("2024-01-01T00:00:00+00:00").unwrap();
        assert_eq!(ts.year(), 2024);
    }

    #[test]
    fn test_parse_timestamp_naive() {
        let ts = parse_timestamp("2024-01-01T00:00:00").unwrap();
        assert_eq!(ts.year(), 2024);
    }

    #[test]
    fn test_parse_timestamp_unix() {
        let ts = parse_timestamp("1704067200").unwrap();
        assert_eq!(ts.year(), 2024);
    }

    #[test]
    fn test_parse_timestamp_unix_millis() {
        let ts = parse_timestamp("1704067200000").unwrap();
        assert_eq!(ts.year(), 2024);
    }

    #[test]
    fn test_parse_range_end_for_date_is_exclusive_next_day() {
        match parse_range_end("2024-01-01").unwrap() {
            RangeEnd::Exclusive(dt) => assert_eq!(dt.day(), 2),
            RangeEnd::Inclusive(_) => panic!("expected exclusive day end"),
        }
    }

    #[test]
    fn test_load_csv_data_in_range_filters_rows() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("candles.csv");
        fs::write(
            &file,
            "timestamp,open,high,low,close,volume,symbol\n\
            2024-01-01T00:00:00+00:00,1,1,1,1,1,BTC\n\
            2024-01-02T00:00:00+00:00,2,2,2,2,2,BTC\n\
            2024-01-03T00:00:00+00:00,3,3,3,3,3,BTC\n",
        )
        .unwrap();

        let candles = load_csv_data_in_range(file.to_str().unwrap(), "2024-01-02", "2024-01-02").unwrap();
        assert_eq!(candles.len(), 1);
        assert_eq!(candles[0].open, 2.0);
    }
}
