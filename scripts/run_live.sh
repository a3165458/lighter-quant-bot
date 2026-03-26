#!/bin/bash
set -e

echo "🚀 启动 Lighter 交易机器人..."

# 加载环境变量
if [ -f .env ]; then
    export $(cat .env | grep -v '^#' | xargs)
fi

# 检查必要的环境变量
if [ -z "$LIGHTER_API_KEY" ] || [ -z "$LIGHTER_SECRET_KEY" ]; then
    echo "❌ 错误: 请设置 LIGHTER_API_KEY 和 LIGHTER_SECRET_KEY"
    echo "💡 请复制 .env.example 为 .env 并填入你的API密钥"
    exit 1
fi

# 构建项目
echo "🔨 构建项目..."
cargo build --release

# 运行机器人
echo "🤖 运行交易机器人..."
RUST_LOG=${RUST_LOG:-info} ./target/release/lighter-bot live --config config/settings.yaml
