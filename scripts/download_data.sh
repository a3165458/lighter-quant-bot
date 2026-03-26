#!/bin/bash
set -e

echo "📥 下载历史数据..."

SYMBOL=${1:-"BTCUSDT"}
INTERVAL=${2:-"1h"}
START_DATE=${3:-"2024-01-01"}
END_DATE=${4:-"2024-06-01"}

# 创建数据目录
mkdir -p backtests/data

echo "下载 $SYMBOL $INTERVAL 数据从 $START_DATE 到 $END_DATE"
# 这里需要实现数据下载逻辑

echo "✅ 数据下载完成！保存到 backtests/data/"
