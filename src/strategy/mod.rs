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

        Ok(Box::new(trend_strategy::TrendStrategy::new(
            fast_ma, slow_ma, stop_loss, take_profit,
        )))
    } else {
        anyhow::bail!("没有启用任何策略")
    }
}

/// 根据策略名称创建策略（用于回测）
pub fn create_strategy_from_name(name: &str) -> Result<Box<dyn Strategy>> {
    match name {
        "grid_trading" | "grid" => {
            Ok(Box::new(grid_strategy::GridStrategy::new(20, 50.0, 0.015)))
        }
        "trend_following" | "trend" => {
            Ok(Box::new(trend_strategy::TrendStrategy::new(7, 21, 0.03, 0.06)))
        }
        _ => anyhow::bail!("未知策略: {}", name),
    }
}
