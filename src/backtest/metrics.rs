use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::info;

use super::engine::BacktestResults;

/// 生成回测报告
pub async fn generate_report(results: &BacktestResults, output_dir: &str) -> Result<()> {
    fs::create_dir_all(output_dir)?;

    // 生成文本摘要
    let summary = format_summary(results);
    let summary_path = Path::new(output_dir).join("summary.txt");
    fs::write(&summary_path, &summary)?;
    info!("报告已保存: {}", summary_path.display());

    // 生成交易记录CSV
    let trades_path = Path::new(output_dir).join("trades.csv");
    write_trades_csv(results, &trades_path)?;

    // 生成权益曲线CSV
    let equity_path = Path::new(output_dir).join("equity_curve.csv");
    write_equity_csv(results, &equity_path)?;

    Ok(())
}

/// 格式化回测摘要
fn format_summary(results: &BacktestResults) -> String {
    format!(
        r#"========================================
    回测报告
========================================

基本信息:
  初始资金:     ${:.2}
  最终资金:     ${:.2}
  总收益率:     {:.2}%

绩效指标:
  夏普比率:     {:.3}
  最大回撤:     {:.2}%
  盈亏比:       {:.2}

交易统计:
  总交易次数:   {}
  盈利交易:     {}
  亏损交易:     {}
  胜率:         {:.1}%
  平均盈利:     ${:.2}
  平均亏损:     ${:.2}

========================================
"#,
        results.initial_capital,
        results.final_capital,
        results.total_return * 100.0,
        results.sharpe_ratio,
        results.max_drawdown * 100.0,
        results.profit_factor,
        results.total_trades,
        results.winning_trades,
        results.losing_trades,
        results.win_rate * 100.0,
        results.avg_profit,
        results.avg_loss,
    )
}

/// 写入交易记录CSV
fn write_trades_csv(results: &BacktestResults, path: &Path) -> Result<()> {
    let mut csv = String::from("timestamp,symbol,side,price,quantity,pnl,commission\n");

    for trade in &results.trades {
        csv.push_str(&format!(
            "{},{},{:?},{:.6},{:.6},{:.6},{:.6}\n",
            trade.timestamp.to_rfc3339(),
            trade.symbol,
            trade.side,
            trade.price,
            trade.quantity,
            trade.pnl,
            trade.commission,
        ));
    }

    fs::write(path, csv)?;
    Ok(())
}

/// 写入权益曲线CSV
fn write_equity_csv(results: &BacktestResults, path: &Path) -> Result<()> {
    let mut csv = String::from("timestamp,equity\n");

    for (timestamp, equity) in &results.equity_curve {
        csv.push_str(&format!(
            "{},{:.2}\n",
            timestamp.to_rfc3339(),
            equity,
        ));
    }

    fs::write(path, csv)?;
    Ok(())
}
