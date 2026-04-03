use anyhow::Result;
use chrono::Utc;
use config::Config;
use tracing::{info, warn};

use crate::lighter::types::{TradeSignal, Position, Side};

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
    /// Per-position stop-loss percentage (e.g., 0.05 = 5%)
    position_stop_loss_pct: f64,
    /// Per-position take-profit percentage (e.g., 0.08 = 8%)
    position_take_profit_pct: f64,
    /// Whether emergency close has been triggered (prevents re-entry)
    emergency_triggered: bool,
    /// Track which UTC day the emergency was triggered
    emergency_day: Option<u32>,
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

        let position_stop_loss_pct = settings
            .get_float("risk.stop_loss.position_stop_loss_percent")
            .unwrap_or(5.0) / 100.0;

        let position_take_profit_pct = settings
            .get_float("risk.stop_loss.position_take_profit_percent")
            .unwrap_or(8.0) / 100.0;

        info!("风控初始化: 最大回撤 {:.1}%, 日亏损限制 {:.1}%, 最大杠杆 {:.0}x, 止损 {:.1}%, 止盈 {:.1}%",
            max_drawdown_pct * 100.0,
            daily_loss_limit_pct * 100.0,
            max_leverage,
            position_stop_loss_pct * 100.0,
            position_take_profit_pct * 100.0,
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
            position_stop_loss_pct,
            position_take_profit_pct,
            emergency_triggered: false,
            emergency_day: None,
        })
    }

    /// 更新当前权益
    #[allow(dead_code)]
    pub fn update_equity(&mut self, equity: f64) {
        self.current_equity = equity;
        // If initial_equity was never set from real data, sync it
        if (self.initial_equity - 10000.0).abs() < 1.0 {
            self.initial_equity = equity;
        }
    }

    /// 更新日PnL
    #[allow(dead_code)]
    pub fn update_daily_pnl(&mut self, pnl: f64) {
        self.current_daily_pnl = pnl;

        // Auto-reset emergency on new UTC day
        if self.emergency_triggered {
            let today = (Utc::now().timestamp() / 86400) as u32;
            if let Some(trigger_day) = self.emergency_day {
                if today > trigger_day {
                    info!("🔄 新的一天 — 重置紧急模式，恢复交易");
                    self.emergency_triggered = false;
                    self.emergency_day = None;
                    self.initial_equity = self.current_equity; // reset baseline
                }
            }
        }
    }

    /// 重置每日PnL（每日开盘时调用）
    #[allow(dead_code)]
    pub fn reset_daily(&mut self) {
        self.current_daily_pnl = 0.0;
    }

    /// 检查交易信号是否通过风控
    pub async fn check_signal(&self, signal: &TradeSignal) -> Result<bool> {
        // Block all new signals if emergency close was triggered
        if self.emergency_triggered {
            warn!("❌ 风控拒绝: 紧急平仓已触发，禁止新交易");
            return Ok(false);
        }

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

        // 检查3：单笔交易大小（杠杆感知：考虑最大杠杆倍数）
        let trade_value = signal.price * signal.quantity;
        let leverage_factor = if self.max_leverage > 1.0 { self.max_leverage } else { 1.0 };
        let max_trade_value = self.current_equity * self.max_single_trade_pct * leverage_factor;
        if trade_value > max_trade_value {
            warn!("❌ 风控拒绝: 交易金额 ${:.2} 超过单笔限制 ${:.2} (equity*{:.0}%*{:.0}x)",
                trade_value, max_trade_value, self.max_single_trade_pct * 100.0, leverage_factor);
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
    pub fn should_emergency_close(&self) -> bool {
        let drawdown = (self.initial_equity - self.current_equity) / self.initial_equity;

        // 超过最大回撤的1.5倍时紧急平仓
        if drawdown >= self.max_drawdown_pct * 1.5 {
            warn!("🚨 紧急平仓触发! 回撤 {:.2}%", drawdown * 100.0);
            return true;
        }

        // 日内亏损超过限制的1.5倍
        let daily_loss = -self.current_daily_pnl / self.initial_equity;
        if daily_loss >= self.daily_loss_limit_pct * 1.5 {
            warn!("🚨 紧急平仓触发! 日内亏损 {:.2}%", daily_loss * 100.0);
            return true;
        }

        false
    }

    /// 标记紧急平仓已触发
    pub fn set_emergency_triggered(&mut self) {
        self.emergency_triggered = true;
        self.emergency_day = Some((Utc::now().timestamp() / 86400) as u32);
    }

    /// 检查是否已触发紧急平仓
    pub fn is_emergency_triggered(&self) -> bool {
        self.emergency_triggered
    }

    /// 检查持仓是否需要止损或止盈平仓
    /// 返回需要平仓的持仓列表，每项包含 (symbol, market_id_hint, side_to_close, size, reason)
    pub fn check_position_stop_loss_take_profit(
        &self,
        positions: &[Position],
        current_prices: &std::collections::HashMap<String, f64>,
    ) -> Vec<PositionCloseSignal> {
        let mut signals = Vec::new();

        for pos in positions {
            if pos.size.abs() < 1e-10 {
                continue;
            }

            let current_price = match current_prices.get(&pos.symbol) {
                Some(&p) if p > 0.0 => p,
                _ => continue,
            };

            let pnl_pct = match pos.side {
                Side::Buy => (current_price - pos.entry_price) / pos.entry_price,
                Side::Sell => (pos.entry_price - current_price) / pos.entry_price,
            };

            // Stop-loss: close if loss exceeds threshold
            if pnl_pct <= -self.position_stop_loss_pct {
                warn!("🛑 止损触发: {} {:?} entry={:.2} now={:.2} pnl={:.2}%",
                    pos.symbol, pos.side, pos.entry_price, current_price, pnl_pct * 100.0);
                signals.push(PositionCloseSignal {
                    symbol: pos.symbol.clone(),
                    side_to_close: match pos.side {
                        Side::Buy => Side::Sell,
                        Side::Sell => Side::Buy,
                    },
                    size: pos.size.abs(),
                    entry_price: pos.entry_price,
                    current_price,
                    pnl_pct,
                    reason: format!("止损: {:.2}% (阈值 -{:.1}%)", pnl_pct * 100.0, self.position_stop_loss_pct * 100.0),
                });
            }

            // Take-profit: close if profit exceeds threshold
            if pnl_pct >= self.position_take_profit_pct {
                info!("🎯 止盈触发: {} {:?} entry={:.2} now={:.2} pnl=+{:.2}%",
                    pos.symbol, pos.side, pos.entry_price, current_price, pnl_pct * 100.0);
                signals.push(PositionCloseSignal {
                    symbol: pos.symbol.clone(),
                    side_to_close: match pos.side {
                        Side::Buy => Side::Sell,
                        Side::Sell => Side::Buy,
                    },
                    size: pos.size.abs(),
                    entry_price: pos.entry_price,
                    current_price,
                    pnl_pct,
                    reason: format!("止盈: +{:.2}% (阈值 +{:.1}%)", pnl_pct * 100.0, self.position_take_profit_pct * 100.0),
                });
            }
        }

        signals
    }

    /// Update risk parameters at runtime from dashboard
    pub fn update_params(
        &mut self,
        max_drawdown_pct: Option<f64>,
        daily_loss_limit_pct: Option<f64>,
        max_leverage: Option<f64>,
        position_stop_loss_pct: Option<f64>,
        position_take_profit_pct: Option<f64>,
    ) {
        if let Some(v) = max_drawdown_pct { self.max_drawdown_pct = v / 100.0; }
        if let Some(v) = daily_loss_limit_pct { self.daily_loss_limit_pct = v / 100.0; }
        if let Some(v) = max_leverage { self.max_leverage = v; }
        if let Some(v) = position_stop_loss_pct { self.position_stop_loss_pct = v / 100.0; }
        if let Some(v) = position_take_profit_pct { self.position_take_profit_pct = v / 100.0; }
        info!("🔧 Risk params updated: drawdown={:.1}%, daily_loss={:.1}%, leverage={:.0}x, sl={:.1}%, tp={:.1}%",
            self.max_drawdown_pct * 100.0, self.daily_loss_limit_pct * 100.0, self.max_leverage,
            self.position_stop_loss_pct * 100.0, self.position_take_profit_pct * 100.0);
    }

    /// Get current risk config as a serializable map
    pub fn get_config(&self) -> serde_json::Value {
        serde_json::json!({
            "max_drawdown_pct": (self.max_drawdown_pct * 100.0),
            "daily_loss_limit_pct": (self.daily_loss_limit_pct * 100.0),
            "max_leverage": self.max_leverage,
            "position_stop_loss_pct": (self.position_stop_loss_pct * 100.0),
            "position_take_profit_pct": (self.position_take_profit_pct * 100.0),
        })
    }

    /// Get current max leverage setting
    pub fn max_leverage(&self) -> f64 {
        self.max_leverage
    }

    /// 获取当前风控状态
    pub fn status(&self) -> RiskStatus {
        let drawdown = (self.initial_equity - self.current_equity) / self.initial_equity;
        let daily_loss = -self.current_daily_pnl / self.initial_equity;

        RiskStatus {
            current_equity: self.current_equity,
            drawdown_pct: drawdown * 100.0,
            daily_loss_pct: daily_loss * 100.0,
            max_drawdown_limit: self.max_drawdown_pct * 100.0,
            daily_loss_limit: self.daily_loss_limit_pct * 100.0,
            position_stop_loss_pct: self.position_stop_loss_pct * 100.0,
            position_take_profit_pct: self.position_take_profit_pct * 100.0,
            is_healthy: drawdown < self.max_drawdown_pct && daily_loss < self.daily_loss_limit_pct && !self.emergency_triggered,
            emergency_triggered: self.emergency_triggered,
        }
    }
}

/// 持仓平仓信号
#[derive(Debug, Clone)]
pub struct PositionCloseSignal {
    pub symbol: String,
    pub side_to_close: Side,
    pub size: f64,
    pub entry_price: f64,
    pub current_price: f64,
    pub pnl_pct: f64,
    pub reason: String,
}

/// 风控状态
#[derive(Debug, Clone, serde::Serialize)]
pub struct RiskStatus {
    pub current_equity: f64,
    pub drawdown_pct: f64,
    pub daily_loss_pct: f64,
    pub max_drawdown_limit: f64,
    pub daily_loss_limit: f64,
    pub position_stop_loss_pct: f64,
    pub position_take_profit_pct: f64,
    pub is_healthy: bool,
    pub emergency_triggered: bool,
}
