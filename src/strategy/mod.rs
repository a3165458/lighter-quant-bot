pub mod dca_strategy;
pub mod grid_strategy;
pub mod trend_strategy;

use anyhow::Result;
use async_trait::async_trait;
use config::Config;

use crate::lighter::types::{MarketSnapshot, TradeSignal};

/// 策略特征
#[async_trait]
pub trait Strategy: Send + Sync {
    /// 策略名称
    #[allow(dead_code)]
    fn name(&self) -> &str;

    /// 评估市场状态，返回交易信号
    async fn evaluate(&self, snapshot: &MarketSnapshot) -> Result<Option<Vec<TradeSignal>>>;

    /// 重置策略状态
    #[allow(dead_code)]
    fn reset(&mut self);

    /// Clear filled/pending state (e.g. after stale orders cancelled).
    /// Uses interior mutability so it can be called via &self / Arc<dyn Strategy>.
    fn clear_filled_state(&self) {}
}

/// 根据配置创建策略
pub fn create_strategy(settings: &Config) -> Result<Box<dyn Strategy>> {
    let grid_enabled = settings
        .get_bool("trading.strategies.grid_trading.enabled")
        .unwrap_or(false);

    let trend_enabled = settings
        .get_bool("trading.strategies.trend_following.enabled")
        .unwrap_or(false);

    if grid_enabled {
        let grid_count = settings.get_int("trading.strategies.grid_trading.grid_count")
            .unwrap_or(10) as usize;
        let investment = settings.get_float("trading.strategies.grid_trading.investment_per_grid")
            .unwrap_or(100.0);
        let deviation = settings.get_float("trading.strategies.grid_trading.price_deviation")
            .unwrap_or(0.02);

        Ok(Box::new(grid_strategy::GridStrategy::new(
            grid_count, investment, deviation,
        )))
    } else if trend_enabled {
        let fast_ma = settings.get_int("trading.strategies.trend_following.fast_ma")
            .unwrap_or(10) as usize;
        let slow_ma = settings.get_int("trading.strategies.trend_following.slow_ma")
            .unwrap_or(30) as usize;
        let stop_loss = settings.get_float("trading.strategies.trend_following.stop_loss")
            .unwrap_or(0.05);
        let take_profit = settings.get_float("trading.strategies.trend_following.take_profit")
            .unwrap_or(0.1);
        let trailing_stop = settings.get_float("trading.strategies.trend_following.trailing_stop")
            .unwrap_or(0.0);
        let notional = settings.get_float("trading.strategies.trend_following.notional")
            .unwrap_or(1000.0);

        Ok(Box::new(trend_strategy::TrendStrategy::with_options(
            fast_ma, slow_ma, stop_loss, take_profit, trailing_stop, notional,
        )))
    } else {
        // Default to grid strategy
        Ok(Box::new(grid_strategy::GridStrategy::new(10, 100.0, 0.02)))
    }
}

/// 根据策略名称创建策略（用于回测）
pub fn create_strategy_from_name(name: &str) -> Result<Box<dyn Strategy>> {
    create_strategy_with_params(name, None)
}

/// 根据策略名和可选参数创建策略
/// params 格式: "grid_count=10,investment=8.0,deviation=0.008"
pub fn create_strategy_with_params(name: &str, params: Option<&str>) -> Result<Box<dyn Strategy>> {
    let kv = parse_params(params.unwrap_or(""));

    match name {
        "grid_trading" | "grid" => {
            let grid_count = kv.get("grid_count").and_then(|v| v.parse().ok()).unwrap_or(10);
            let investment = kv.get("investment_per_grid")
                .or_else(|| kv.get("investment"))
                .and_then(|v| v.parse().ok()).unwrap_or(8.0);
            let deviation = kv.get("price_deviation")
                .or_else(|| kv.get("deviation"))
                .and_then(|v| v.parse().ok()).unwrap_or(0.008);
            Ok(Box::new(grid_strategy::GridStrategy::new(grid_count, investment, deviation)))
        }
        "trend_following" | "trend" => {
            let fast_ma = kv.get("fast_ma").and_then(|v| v.parse().ok()).unwrap_or(7);
            let slow_ma = kv.get("slow_ma").and_then(|v| v.parse().ok()).unwrap_or(21);
            let stop_loss = kv.get("stop_loss").and_then(|v| v.parse().ok()).unwrap_or(0.03);
            let take_profit = kv.get("take_profit").and_then(|v| v.parse().ok()).unwrap_or(0.06);
            let trailing_stop = kv.get("trailing_stop").and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let notional = kv.get("notional").and_then(|v| v.parse().ok()).unwrap_or(1000.0);
            Ok(Box::new(trend_strategy::TrendStrategy::with_options(
                fast_ma, slow_ma, stop_loss, take_profit, trailing_stop, notional,
            )))
        }
        "dca" => {
            let interval = kv.get("interval").and_then(|v| v.parse().ok()).unwrap_or(4.0);
            let amount = kv.get("amount").and_then(|v| v.parse().ok()).unwrap_or(5.0);
            let dip = kv.get("dip_threshold").and_then(|v| v.parse().ok()).unwrap_or(2.0);
            Ok(Box::new(dca_strategy::DcaStrategy::new(interval, amount, dip)))
        }
        _ => anyhow::bail!("未知策略: {}", name),
    }
}

fn parse_params(s: &str) -> std::collections::HashMap<String, String> {
    s.split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((parts.next()?.trim().to_string(), parts.next()?.trim().to_string()))
        })
        .collect()
}
