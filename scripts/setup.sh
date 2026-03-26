#!/bin/bash
set -e

echo "🔧 初始化开发环境..."

# 检查 Rust 是否安装
if ! command -v cargo &> /dev/null; then
    echo "❌ Rust 未安装，正在安装..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

echo "✅ Rust 版本: $(rustc --version)"
echo "✅ Cargo 版本: $(cargo --version)"

# 创建目录结构
mkdir -p backtests/{data,results}
mkdir -p config/strategies
mkdir -p logs

# 复制环境变量模板
if [ ! -f .env ]; then
    cp .env.example .env
    echo "💡 请编辑 .env 文件填入你的 API 密钥"
fi

# 构建项目
echo "🔨 构建项目..."
cargo build

echo "✅ 开发环境初始化完成！"
echo ""
echo "下一步："
echo "  1. 编辑 .env 文件填入 API 密钥"
echo "  2. 运行 cargo test 验证项目"
echo "  3. 运行 ./scripts/run_backtest.sh 测试回测"
