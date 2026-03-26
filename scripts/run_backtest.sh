#!/bin/bash
set -e

echo "📊 运行回测..."

# 默认参数
STRATEGY=${1:-"grid_trading"}
DATA_FILE=${2:-"backtests/data/BTCUSDT-1h.csv"}
START_DATE=${3:-"2024-01-01"}
END_DATE=${4:-"2024-06-01"}

# 构建回测命令
cargo run --release -- \
    backtest \
    --strategy "$STRATEGY" \
    --data "$DATA_FILE" \
    --start "$START_DATE" \
    --end "$END_DATE" \
    --capital 10000 \
    --output "backtests/results/$(date +%Y%m%d-%H%M%S)"

echo "✅ 回测完成！结果保存在 backtests/results/ 目录"
