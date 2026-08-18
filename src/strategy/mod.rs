pub mod dca_strategy;
pub mod grid_strategy;
pub mod inventory_bias;
pub mod market_making;
pub mod rolling;
pub mod trend_strategy;
pub mod vol_obi;

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
        let grid_count = settings
            .get_int("trading.strategies.grid_trading.grid_count")
            .unwrap_or(10) as usize;
        let investment = settings
            .get_float("trading.strategies.grid_trading.investment_per_grid")
            .unwrap_or(100.0);
        let deviation = settings
            .get_float("trading.strategies.grid_trading.price_deviation")
            .unwrap_or(0.02);

        // 实盘路径：yaml 可选覆盖库存政策，但 research_nocap 一律拒绝
        let mode_raw = settings
            .get_string("trading.strategies.grid_trading.inventory_mode")
            .unwrap_or_else(|_| "hard".to_string());
        reject_live_research_nocap(&mode_raw, "inventory_mode")?;
        let mode = grid_strategy::InventoryMode::parse(&mode_raw)?;
        let soft_cap = settings
            .get_float("trading.strategies.grid_trading.soft_cap_grids")
            .ok();
        let hard_cap = settings
            .get_float("trading.strategies.grid_trading.hard_cap_grids")
            .ok();

        Ok(Box::new(grid_strategy::GridStrategy::with_inventory(
            grid_count, investment, deviation, mode, soft_cap, hard_cap,
        )?))
    } else if trend_enabled {
        let fast_ma = settings
            .get_int("trading.strategies.trend_following.fast_ma")
            .unwrap_or(10) as usize;
        let slow_ma = settings
            .get_int("trading.strategies.trend_following.slow_ma")
            .unwrap_or(30) as usize;
        let stop_loss = settings
            .get_float("trading.strategies.trend_following.stop_loss")
            .unwrap_or(0.05);
        let take_profit = settings
            .get_float("trading.strategies.trend_following.take_profit")
            .unwrap_or(0.1);
        let trailing_stop = settings
            .get_float("trading.strategies.trend_following.trailing_stop")
            .unwrap_or(0.0);
        let notional = settings
            .get_float("trading.strategies.trend_following.notional")
            .unwrap_or(1000.0);
        let adx_threshold = settings
            .get_float("trading.strategies.trend_following.adx_threshold")
            .unwrap_or(0.0);
        let adx_period = settings
            .get_int("trading.strategies.trend_following.adx_period")
            .unwrap_or(14) as usize;

        let slow_period_read = slow_ma;
        let confirm_min = settings
            .get_float("trading.strategies.trend_following.confirm_slope_min")
            .unwrap_or(0.0);
        let confirm_lookback = settings
            .get_int("trading.strategies.trend_following.confirm_lookback")
            .unwrap_or((slow_period_read / 2).max(1) as i64)
            as usize;

        Ok(Box::new(
            trend_strategy::TrendStrategy::with_options(
                fast_ma,
                slow_ma,
                stop_loss,
                take_profit,
                trailing_stop,
                notional,
            )
            .with_adx_filter(adx_threshold, adx_period)
            .with_slope_confirm(confirm_min, confirm_lookback),
        ))
    } else if settings
        .get_bool("trading.strategies.market_making.enabled")
        .unwrap_or(false)
    {
        Ok(Box::new(build_mm_from_settings(settings)?))
    } else {
        // Default to grid strategy
        Ok(Box::new(grid_strategy::GridStrategy::new(10, 100.0, 0.02)))
    }
}

fn reject_live_research_nocap(raw: &str, field: &str) -> Result<()> {
    if raw.trim().eq_ignore_ascii_case("research_nocap") {
        anyhow::bail!("实盘配置不允许 {field}=research_nocap（研究专用）");
    }
    Ok(())
}

fn mm_params_from_settings(settings: &Config) -> market_making::MmQuoteParams {
    market_making::MmQuoteParams {
        bid_spread: settings
            .get_float("trading.strategies.market_making.bid_spread")
            .unwrap_or(0.001),
        ask_spread: settings
            .get_float("trading.strategies.market_making.ask_spread")
            .unwrap_or(0.001),
        order_notional: settings
            .get_float("trading.strategies.market_making.order_notional")
            .or_else(|_| settings.get_float("trading.strategies.market_making.order_amount"))
            .unwrap_or(50.0),
        inventory_skew: settings
            .get_float("trading.strategies.market_making.inventory_skew")
            .unwrap_or(0.5),
        inventory_target: settings
            .get_float("trading.strategies.market_making.inventory_target")
            .unwrap_or(0.01),
        max_inventory: settings
            .get_float("trading.strategies.market_making.max_inventory")
            .unwrap_or(0.05),
        min_requote_secs: settings
            .get_int("trading.strategies.market_making.min_requote_secs")
            .unwrap_or(10),
    }
}

fn vol_mm_settings_from_config(settings: &Config) -> market_making::VolObiMmSettings {
    let mut cfg = market_making::VolObiMmSettings::default();
    if let Ok(raw) = settings.get_string("trading.strategies.market_making.alpha_source") {
        if let Ok(src) = market_making::AlphaSource::parse(&raw) {
            cfg.alpha_source = src;
        }
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.alpha_stale_secs") {
        cfg.alpha_stale_secs = v;
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.vol_obi.window_steps") {
        cfg.vol_obi.window_steps = v.max(2.0) as usize;
    }
    if let Ok(v) = settings.get_int("trading.strategies.market_making.vol_obi.step_ns") {
        cfg.vol_obi.step_ns = v.max(1);
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.vol_obi.vol_to_half_spread")
    {
        cfg.vol_obi.vol_to_half_spread = v;
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.vol_obi.min_half_spread_bps")
    {
        cfg.vol_obi.min_half_spread_bps = v;
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.vol_obi.c1_ticks") {
        cfg.vol_obi.c1_ticks = v;
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.vol_obi.skew") {
        cfg.vol_obi.skew = v;
    }
    if let Ok(v) = settings.get_float("trading.strategies.market_making.vol_obi.looking_depth") {
        cfg.vol_obi.looking_depth = v;
    }
    if let Ok(v) = settings.get_int("trading.strategies.market_making.vol_obi.min_warmup_samples") {
        cfg.vol_obi.min_warmup_samples = v.max(1);
    }
    cfg
}

fn quote_engine_from_settings(settings: &Config) -> market_making::QuoteEngine {
    settings
        .get_string("trading.strategies.market_making.quote_engine")
        .ok()
        .and_then(|raw| market_making::QuoteEngine::parse(&raw).ok())
        .unwrap_or(market_making::QuoteEngine::VolObi)
}

fn build_mm_from_settings(settings: &Config) -> Result<market_making::MarketMakingStrategy> {
    let mode_raw = settings
        .get_string("trading.strategies.market_making.inventory_mode")
        .unwrap_or_else(|_| "hard".to_string());
    reject_live_research_nocap(&mode_raw, "inventory_mode")?;
    let mode = grid_strategy::InventoryMode::parse(&mode_raw)?;
    market_making::MarketMakingStrategy::with_engine(
        mm_params_from_settings(settings),
        mode,
        quote_engine_from_settings(settings),
        vol_mm_settings_from_config(settings),
    )
}

/// 根据策略名称创建策略（用于回测）
#[allow(dead_code)]
pub fn create_strategy_from_name(name: &str) -> Result<Box<dyn Strategy>> {
    create_strategy_with_params(name, None)
}

/// 根据策略名和可选参数创建策略
/// params 格式: "grid_count=10,investment=8.0,deviation=0.008"
pub fn create_strategy_with_params(name: &str, params: Option<&str>) -> Result<Box<dyn Strategy>> {
    let kv = parse_params(params.unwrap_or(""));

    match name {
        "grid_trading" | "grid" => {
            let grid_count = kv
                .get("grid_count")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10);
            let investment = kv
                .get("investment_per_grid")
                .or_else(|| kv.get("investment"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(8.0);
            let deviation = kv
                .get("price_deviation")
                .or_else(|| kv.get("deviation"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.008);

            // 库存政策（回测/研究）。未指定时保持原行为 = 实盘硬上限。
            let mode_raw = kv.get("inventory_mode").map(|s| s.as_str());
            let soft_cap = kv
                .get("soft_cap")
                .or_else(|| kv.get("soft_cap_grids"))
                .map(|v| v.parse::<f64>())
                .transpose()
                .map_err(|e| anyhow::anyhow!("soft_cap 解析失败: {e}"))?;
            let hard_cap = kv
                .get("hard_cap")
                .or_else(|| kv.get("hard_cap_grids"))
                .map(|v| v.parse::<f64>())
                .transpose()
                .map_err(|e| anyhow::anyhow!("hard_cap 解析失败: {e}"))?;

            match mode_raw {
                None if soft_cap.is_none() && hard_cap.is_none() => Ok(Box::new(
                    grid_strategy::GridStrategy::new(grid_count, investment, deviation),
                )),
                _ => {
                    let mode = grid_strategy::InventoryMode::parse(mode_raw.unwrap_or("hard"))?;
                    Ok(Box::new(grid_strategy::GridStrategy::with_inventory(
                        grid_count, investment, deviation, mode, soft_cap, hard_cap,
                    )?))
                }
            }
        }
        "trend_following" | "trend" => {
            let fast_ma = kv.get("fast_ma").and_then(|v| v.parse().ok()).unwrap_or(7);
            let slow_ma = kv.get("slow_ma").and_then(|v| v.parse().ok()).unwrap_or(21);
            let stop_loss = kv
                .get("stop_loss")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.03);
            let take_profit = kv
                .get("take_profit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.06);
            let trailing_stop = kv
                .get("trailing_stop")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let notional = kv
                .get("notional")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000.0);
            let adx_threshold = kv
                .get("adx_threshold")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let adx_period = kv
                .get("adx_period")
                .and_then(|v| v.parse().ok())
                .unwrap_or(14);
            let confirm_min = kv
                .get("confirm_slope_min")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            let confirm_lookback = kv
                .get("confirm_lookback")
                .and_then(|v| v.parse().ok())
                .unwrap_or((slow_ma / 2).max(1));
            Ok(Box::new(
                trend_strategy::TrendStrategy::with_options(
                    fast_ma,
                    slow_ma,
                    stop_loss,
                    take_profit,
                    trailing_stop,
                    notional,
                )
                .with_adx_filter(adx_threshold, adx_period)
                .with_slope_confirm(confirm_min, confirm_lookback),
            ))
        }
        "dca" => {
            let interval = kv
                .get("interval")
                .and_then(|v| v.parse().ok())
                .unwrap_or(4.0);
            let amount = kv.get("amount").and_then(|v| v.parse().ok()).unwrap_or(5.0);
            let dip = kv
                .get("dip_threshold")
                .and_then(|v| v.parse().ok())
                .unwrap_or(2.0);
            Ok(Box::new(dca_strategy::DcaStrategy::new(
                interval, amount, dip,
            )))
        }
        "market_making" | "mm" => {
            let defaults = market_making::MmQuoteParams::default();
            let params = market_making::MmQuoteParams {
                bid_spread: kv
                    .get("bid_spread")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.bid_spread),
                ask_spread: kv
                    .get("ask_spread")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.ask_spread),
                order_notional: kv
                    .get("order_notional")
                    .or_else(|| kv.get("order_amount"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.order_notional),
                inventory_skew: kv
                    .get("inventory_skew")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.inventory_skew),
                inventory_target: kv
                    .get("inventory_target")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.inventory_target),
                max_inventory: kv
                    .get("max_inventory")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.max_inventory),
                min_requote_secs: kv
                    .get("min_requote_secs")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.min_requote_secs),
            };
            let mode = grid_strategy::InventoryMode::parse(
                kv.get("inventory_mode")
                    .map(|s| s.as_str())
                    .unwrap_or("hard"),
            )?;
            let engine = kv
                .get("quote_engine")
                .map(|s| s.as_str())
                .map(market_making::QuoteEngine::parse)
                .transpose()?
                .unwrap_or(market_making::QuoteEngine::VolObi);
            let mut vol = market_making::VolObiMmSettings::default();
            if let Some(raw) = kv.get("alpha_source") {
                vol.alpha_source = market_making::AlphaSource::parse(raw)?;
            }
            if engine == market_making::QuoteEngine::Simple {
                vol.alpha_source = market_making::AlphaSource::Local;
            }
            if let Some(v) = kv
                .get("min_half_spread_bps")
                .and_then(|s| s.parse::<f64>().ok())
            {
                vol.vol_obi.min_half_spread_bps = v;
            }
            if let Some(v) = kv
                .get("vol_to_half_spread")
                .and_then(|s| s.parse::<f64>().ok())
            {
                vol.vol_obi.vol_to_half_spread = v;
            }
            if let Some(v) = kv
                .get("min_warmup_samples")
                .and_then(|s| s.parse::<i64>().ok())
            {
                vol.vol_obi.min_warmup_samples = v.max(1);
            }
            Ok(Box::new(market_making::MarketMakingStrategy::with_engine(
                params, mode, engine, vol,
            )?))
        }
        _ => anyhow::bail!("未知策略: {}", name),
    }
}

#[cfg(test)]
#[path = "params_tests.rs"]
mod params_tests;

fn parse_params(s: &str) -> std::collections::HashMap<String, String> {
    s.split(',')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((
                parts.next()?.trim().to_string(),
                parts.next()?.trim().to_string(),
            ))
        })
        .collect()
}
