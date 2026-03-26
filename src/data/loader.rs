use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc, Duration};
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

    // 尝试 Unix 时间戳（秒）
    if let Ok(ts) = s.parse::<i64>() {
        if let Some(dt) = DateTime::from_timestamp(ts, 0) {
            return Ok(dt);
        }
    }

    // 尝试 Unix 时间戳（毫秒）
    if let Ok(ts) = s.parse::<i64>() {
        if let Some(dt) = DateTime::from_timestamp(ts / 1000, ((ts % 1000) * 1_000_000) as u32) {
            return Ok(dt);
        }
    }

    anyhow::bail!("无法解析时间戳: {}", s)
}

/// 生成合成的测试数据
pub fn generate_synthetic_data(symbol: &str, days: u32) -> Result<()> {
    use rand::Rng;

    let mut rng = rand::thread_rng();
    let mut price = 10000.0_f64;
    let mut candles = Vec::new();

    let start = Utc::now() - Duration::days(days as i64);
    let total_hours = days * 24;

    for i in 0..total_hours {
        let timestamp = start + Duration::hours(i as i64);

        // 随机游走
        let change = rng.gen_range(-0.02..0.02);
        price *= 1.0 + change;

        let open = price * (1.0 + rng.gen_range(-0.001..0.001));
        let close = price;
        let high = f64::max(open, close) * (1.0 + rng.gen_range(0.0..0.02));
        let low = f64::min(open, close) * (1.0 - rng.gen_range(0.0..0.02));
        let volume: f64 = rng.gen_range(100.0..10000.0);

        candles.push(format!(
            "{},{:.2},{:.2},{:.2},{:.2},{:.4},{}",
            timestamp.to_rfc3339(),
            open, high, low, close, volume, symbol
        ));
    }

    // 写入文件
    let dir = "backtests/data";
    fs::create_dir_all(dir)?;

    let filename = format!("{}/{}-1h.csv", dir, symbol);
    let mut content = String::from("timestamp,open,high,low,close,volume,symbol\n");
    for line in &candles {
        content.push_str(line);
        content.push('\n');
    }

    fs::write(&filename, content)?;
    tracing::info!("生成测试数据: {} ({} 条记录)", filename, candles.len());

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
}
