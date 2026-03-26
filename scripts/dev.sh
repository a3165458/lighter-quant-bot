#!/bin/bash
# 开发环境管理脚本

case "$1" in
    "build")
        echo "🔨 构建项目..."
        cargo build
        ;;
    
    "test")
        echo "🧪 运行测试..."
        cargo test
        ;;
    
    "bench")
        echo "⏱️  运行基准测试..."
        cargo bench
        ;;
    
    "lint")
        echo "🔍 运行代码检查..."
        cargo clippy -- -D warnings
        cargo fmt --check
        ;;
    
    "clean")
        echo "🧹 清理构建缓存..."
        cargo clean
        rm -rf target/
        rm -rf logs/*
        rm -rf backtests/results/*
        ;;
    
    "doc")
        echo "📚 生成文档..."
        cargo doc --open
        ;;
    
    "dashboard")
        echo "🌐 启动监控面板..."
        cargo run -- dashboard --port 3000
        ;;
    
    *)
        echo "用法: $0 {build|test|bench|lint|clean|doc|dashboard}"
        exit 1
        ;;
esac
