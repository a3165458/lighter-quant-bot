use anyhow::{Context, Result};
use config::Config;
use serde::Deserialize;

/// 应用配置
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AppConfig {
    pub lighter: LighterConfig,
    pub trading: TradingConfig,
    pub risk: RiskConfig,
    pub dashboard: DashboardConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LighterConfig {
    pub api_key: String,
    pub secret_key: String,
    pub networks: NetworksConfig,
    pub connection: ConnectionConfig,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct NetworksConfig {
    pub testnet: NetworkEndpoint,
    pub mainnet: NetworkEndpoint,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct NetworkEndpoint {
    pub rest_url: String,
    pub ws_url: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct ConnectionConfig {
    pub timeout_ms: u64,
    pub retry_count: u32,
    pub retry_delay_ms: u64,
    pub heartbeat_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TradingConfig {
    pub symbols: Vec<String>,
    pub strategies: StrategiesConfig,
    pub position: PositionConfig,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct StrategiesConfig {
    pub grid_trading: GridTradingConfig,
    pub trend_following: TrendFollowingConfig,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GridTradingConfig {
    pub enabled: bool,
    pub grid_count: usize,
    pub investment_per_grid: f64,
    pub price_deviation: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TrendFollowingConfig {
    pub enabled: bool,
    pub fast_ma: usize,
    pub slow_ma: usize,
    pub stop_loss: f64,
    pub take_profit: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PositionConfig {
    pub max_single_trade_percent: f64,
    pub max_total_position_percent: f64,
    pub risk_per_trade_percent: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RiskConfig {
    pub stop_loss: StopLossConfig,
    pub position_limit: PositionLimitConfig,
    pub monitoring_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct StopLossConfig {
    pub enabled: bool,
    pub max_drawdown_percent: f64,
    pub daily_loss_limit_percent: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct PositionLimitConfig {
    pub max_leverage: f64,
    pub max_position_size: f64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct DashboardConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub refresh_interval_ms: u64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LoggingConfig {
    pub level: String,
    pub file: String,
    pub rotation: String,
    pub format: String,
}

/// 加载配置文件
#[allow(dead_code)]
pub fn load_config(path: &str) -> Result<AppConfig> {
    let settings = Config::builder()
        .add_source(config::File::with_name(path))
        .add_source(config::Environment::with_prefix("LIGHTER"))
        .build()
        .context("加载配置文件失败")?;

    let config: AppConfig = settings
        .try_deserialize()
        .context("解析配置失败")?;

    Ok(config)
}
