#!/bin/bash
set -e

echo "🚀 启动 Lighter 交易机器人..."

NETWORK="${1:-mainnet}"
case "$NETWORK" in
    mainnet) CONFIG="config/settings.yaml" ;;
    robinhood) CONFIG="config/settings.robinhood.yaml" ;;
    *) echo "❌ 用法: $0 [mainnet|robinhood]"; exit 1 ;;
esac

# 构建项目
echo "🔨 构建项目..."
cargo build --release

# 运行机器人
echo "🤖 运行交易机器人..."
RUST_LOG=${RUST_LOG:-info} ./target/release/lighter-bot live --config "$CONFIG"
