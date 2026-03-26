use anyhow::Result;
use config::Config;
use tracing::{info, warn};

use crate::lighter::types::TradeSignal;

/// 风险管理器
pub struct RiskManager {
    max_drawdown_pct: f64,
    daily_loss_limit_pct: f64,
    #[allow(dead_code)]
    max_leverage: f64,
    max_position_size: f64,
    max_single_trade_pct: f64,
    #[allow(dead_code)]
    max_total_position_pct: f64,
    current_daily_pnl: f64,
    current_equity: f64,
    initial_equity: f64,
}

impl RiskManager {
    /// 从配置文件创建风险管理器
    pub fn new(settings: &Config) -> Result<Self> {
        let max_drawdown_pct = settings
            .get_float("risk.stop_loss.max_drawdown_percent")
            .unwrap_or(10.0) / 100.0;

        let daily_loss_limit_pct = settings
            .get_float("risk.stop_loss.daily_loss_limit_percent")
            .unwrap_or(5.0) / 100.0;

        let max_leverage = settings
            .get_float("risk.position_limit.max_leverage")
            .unwrap_or(3.0);

        let max_position_size = settings
            .get_float("risk.position_limit.max_position_size")
            .unwrap_or(10000.0);

        let max_single_trade_pct = settings
            .get_float("trading.position.max_single_trade_percent")
            .unwrap_or(10.0) / 100.0;

        let max_total_position_pct = settings
            .get_float("trading.position.max_total_position_percent")
            .unwrap_or(50.0) / 100.0;

        info!("风控初始化: 最大回撤 {:.1}%, 日亏损限制 {:.1}%, 最大杠杆 {:.0}x",
            max_drawdown_pct * 100.0,
            daily_loss_limit_pct * 100.0,
            max_leverage
        );

        Ok(Self {
            max_drawdown_pct,
            daily_loss_limit_pct,
            max_leverage,
            max_position_size,
            max_single_trade_pct,
            max_total_position_pct,
            current_daily_pnl: 0.0,
            current_equity: 10000.0,
            initial_equity: 10000.0,
        })
    }

    /// 更新当前权益
    #[allow(dead_code)]
    pub fn update_equity(&mut self, equity: f64) {
        self.current_equity = equity;
    }

    /// 更新日PnL
    #[allow(dead_code)]
    pub fn update_daily_pnl(&mut self, pnl: f64) {
        self.current_daily_pnl = pnl;
    }

    /// 重置每日PnL（每日开盘时调用）
    #[allow(dead_code)]
    pub fn reset_daily(&mut self) {
        self.current_daily_pnl = 0.0;
    }

    /// 检查交易信号是否通过风控
    pub async fn check_signal(&self, signal: &TradeSignal) -> Result<bool> {
        // 检查1：每日亏损限制
        let daily_loss = -self.current_daily_pnl / self.initial_equity;
        if daily_loss >= self.daily_loss_limit_pct {
            warn!("❌ 风控拒绝: 已达到每日亏损限制 ({:.2}%)", daily_loss * 100.0);
            return Ok(false);
        }

        // 检查2：最大回撤
        let drawdown = (self.initial_equity - self.current_equity) / self.initial_equity;
        if drawdown >= self.max_drawdown_pct {
            warn!("❌ 风控拒绝: 已超过最大回撤限制 ({:.2}%)", drawdown * 100.0);
            return Ok(false);
        }

        // 检查3：单笔交易大小
        let trade_value = signal.price * signal.quantity;
        let max_trade_value = self.current_equity * self.max_single_trade_pct;
        if trade_value > max_trade_value {
            warn!("❌ 风控拒绝: 交易金额 ${:.2} 超过单笔限制 ${:.2}",
                trade_value, max_trade_value);
            return Ok(false);
        }

        // 检查4：持仓大小
        if trade_value > self.max_position_size {
            warn!("❌ 风控拒绝: 交易金额 ${:.2} 超过最大持仓限制 ${:.2}",
                trade_value, self.max_position_size);
            return Ok(false);
        }

        Ok(true)
    }

    /// 检查是否需要紧急平仓
    #[allow(dead_code)]
    pub fn should_emergency_close(&self) -> bool {
        let drawdown = (self.initial_equity - self.current_equity) / self.initial_equity;

        // 超过最大回撤的1.5倍时紧急平仓
        if drawdown >= self.max_drawdown_pct * 1.5 {
            warn!("🚨 紧急平仓触发! 回撤 {:.2}%", drawdown * 100.0);
            return true;
        }

        false
    }

    /// 获取当前风控状态
    #[allow(dead_code)]
    pub fn status(&self) -> RiskStatus {
        let drawdown = (self.initial_equity - self.current_equity) / self.initial_equity;
        let daily_loss = -self.current_daily_pnl / self.initial_equity;

        RiskStatus {
            current_equity: self.current_equity,
            drawdown_pct: drawdown * 100.0,
            daily_loss_pct: daily_loss * 100.0,
            max_drawdown_limit: self.max_drawdown_pct * 100.0,
            daily_loss_limit: self.daily_loss_limit_pct * 100.0,
            is_healthy: drawdown < self.max_drawdown_pct && daily_loss < self.daily_loss_limit_pct,
        }
    }
}

/// 风控状态
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code)]
pub struct RiskStatus {
    pub current_equity: f64,
    pub drawdown_pct: f64,
    pub daily_loss_pct: f64,
    pub max_drawdown_limit: f64,
    pub daily_loss_limit: f64,
    pub is_healthy: bool,
}
