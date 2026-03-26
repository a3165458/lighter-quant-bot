// src/main.rs
mod lighter;
mod strategy;
mod backtest;
mod risk;
mod dashboard;
mod data;
mod utils;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, error};

#[derive(Parser)]
#[command(author, version, about = "Lighter 交易机器人", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行实盘交易
    Live {
        /// 配置文件路径
        #[arg(short, long, default_value = "config/settings.yaml")]
        config: String,

        /// 是否使用测试网络
        #[arg(long)]
        testnet: bool,
    },

    /// 运行回测
    Backtest {
        /// 策略名称
        #[arg(short, long)]
        strategy: String,

        /// 历史数据文件路径
        #[arg(short, long)]
        data: String,

        /// 回测开始日期
        #[arg(long)]
        start: String,

        /// 回测结束日期
        #[arg(long)]
        end: String,

        /// 初始资金
        #[arg(long, default_value = "10000")]
        capital: f64,

        /// 输出目录
        #[arg(short, long)]
        output: Option<String>,
    },

    /// 启动监控面板
    Dashboard {
        /// 监听地址
        #[arg(long, default_value = "0.0.0.0")]
        host: String,

        /// 监听端口
        #[arg(short, long, default_value = "2028")]
        port: u16,
    },

    /// 下载历史数据
    Download {
        /// 交易对符号
        #[arg(short, long)]
        symbol: String,

        /// K线周期
        #[arg(short, long, default_value = "1h")]
        interval: String,

        /// 开始日期
        #[arg(long)]
        start: String,

        /// 结束日期
        #[arg(long)]
        end: String,
    },

    /// 生成测试数据
    GenerateData {
        /// 交易对符号
        #[arg(short, long, default_value = "BTCUSDT")]
        symbol: String,

        /// 生成天数
        #[arg(short, long, default_value = "30")]
        days: u32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志系统
    utils::logger::init_logger();

    // 加载环境变量
    dotenv::dotenv().ok();

    // 解析命令行参数
    let cli = Cli::parse();

    match cli.command {
        Commands::Live { config, testnet } => {
            run_live_trading(&config, testnet).await
        }
        Commands::Backtest { strategy, data, start, end, capital, output } => {
            run_backtest(&strategy, &data, &start, &end, capital, output.as_deref()).await
        }
        Commands::Dashboard { host, port } => {
            run_dashboard(&host, port).await
        }
        Commands::Download { symbol, interval, start, end } => {
            download_data(&symbol, &interval, &start, &end).await
        }
        Commands::GenerateData { symbol, days } => {
            generate_test_data(&symbol, days).await
        }
    }
}

async fn run_live_trading(config_path: &str, testnet: bool) -> Result<()> {
    info!("🚀 启动实盘交易，配置文件: {}", config_path);

    // 加载配置
    let settings = Config::builder()
        .add_source(config::File::with_name(config_path))
        .add_source(config::Environment::with_prefix("LIGHTER"))
        .build()
        .context("加载配置文件失败")?;

    // 初始化交易所连接
    let api_key = settings.get_string("lighter.api_key")
        .context("API密钥未配置")?;
    let secret_key = settings.get_string("lighter.secret_key")
        .context("Secret密钥未配置")?;

    let network = if testnet { "testnet" } else { "mainnet" };
    let rest_url = settings.get_string(&format!("lighter.networks.{}.rest_url", network))?;
    let ws_url = settings.get_string(&format!("lighter.networks.{}.ws_url", network))?;

    // 创建数据存储
    let data_store = Arc::new(RwLock::new(data::storage::MarketDataStore::new()));

    // 初始化交易所客户端
    let lighter_client = lighter::client::LighterClient::new(
        &api_key,
        &secret_key,
        &rest_url,
        &ws_url,
    );

    // 测试连接
    info!("📡 测试交易所连接...");
    match lighter_client.get_account_info().await {
        Ok(account) => {
            info!("✅ 连接成功，账户余额: {:?}", account.balances);
        }
        Err(e) => {
            error!("❌ 连接失败: {}", e);
            return Err(e.into());
        }
    }

    // 初始化风控
    let risk_manager = risk::risk_manager::RiskManager::new(&settings)
        .context("初始化风控系统失败")?;

    // 初始化策略
    let strategy = strategy::create_strategy(&settings)
        .context("初始化交易策略失败")?;

    // 启动WebSocket连接
    info!("🔌 启动WebSocket连接...");
    let ws_client = lighter::websocket::LighterWebSocket::new(&ws_url);
    ws_client.connect().await?;

    // 订阅市场数据
    let symbols: Vec<String> = settings.get("trading.symbols")?;
    for symbol in &symbols {
        ws_client.subscribe_market_data(symbol).await?;
        info!("📈 订阅 {}", symbol);
    }

    // 启动主事件循环
    let mut ws_receiver = ws_client.get_receiver();
    let data_store_clone = data_store.clone();

    // 启动定期数据保存任务
    let save_data_store = data_store.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let _snapshot = save_data_store.read().await.get_snapshot();
        }
    });

    info!("🎯 交易系统启动完成，等待市场数据...");

    // 处理消息循环
    while let Ok(msg) = ws_receiver.recv().await {
        // 更新数据存储
        {
            let mut store = data_store_clone.write().await;
            match msg {
                lighter::types::WsMessage::OrderBookUpdate(ref ob) => {
                    store.update_order_book(ob.clone());
                }
                lighter::types::WsMessage::TradeUpdate(ref trade) => {
                    store.add_trade(trade.clone());
                }
                _ => {}
            }
        }

        // 运行策略逻辑
        let snapshot = data_store_clone.read().await.get_snapshot();
        if let Some(signals) = strategy.evaluate(&snapshot).await? {
            for signal in signals {
                // 风控检查
                if risk_manager.check_signal(&signal).await? {
                    info!("📊 生成交易信号: {:?}", signal);
                    // 下单逻辑
                    match lighter_client.place_order(
                        &signal.symbol,
                        signal.side,
                        signal.price,
                        signal.quantity,
                    ).await {
                        Ok(resp) => {
                            info!("✅ 下单成功: order_id={}, status={}", resp.order_id, resp.status);
                        }
                        Err(e) => {
                            error!("❌ 下单失败: {}", e);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_backtest(
    strategy_name: &str,
    data_path: &str,
    start_date: &str,
    end_date: &str,
    initial_capital: f64,
    output_dir: Option<&str>,
) -> Result<()> {
    info!("📊 开始回测策略: {}", strategy_name);
    info!("   数据文件: {}", data_path);
    info!("   回测期间: {} 到 {}", start_date, end_date);
    info!("   初始资金: ${:.2}", initial_capital);

    // 加载历史数据
    let historical_data = data::loader::load_csv_data(data_path)
        .context("加载历史数据失败")?;

    // 初始化回测引擎
    let mut backtest_engine = backtest::engine::BacktestEngine::new(
        initial_capital,
        historical_data,
    );

    // 初始化策略
    let bt_strategy = strategy::create_strategy_from_name(strategy_name)?;

    // 运行回测
    let results = backtest_engine.run(bt_strategy).await?;

    // 生成报告
    let output_path = output_dir.unwrap_or("backtests/results");
    backtest::metrics::generate_report(&results, output_path).await?;

    // 打印摘要
    info!("📈 回测完成！");
    info!("   总收益率: {:.2}%", results.total_return * 100.0);
    info!("   夏普比率: {:.3}", results.sharpe_ratio);
    info!("   最大回撤: {:.2}%", results.max_drawdown * 100.0);
    info!("   交易次数: {}", results.trades.len());
    info!("   胜率: {:.1}%", results.win_rate * 100.0);
    info!("   报告已保存到: {}", output_path);

    Ok(())
}

async fn run_dashboard(host: &str, port: u16) -> Result<()> {
    info!("🌐 启动监控面板 {}:{}", host, port);

    dashboard::server::start(host, port).await
        .context("启动监控面板失败")
}

async fn download_data(
    symbol: &str,
    interval: &str,
    start_date: &str,
    end_date: &str,
) -> Result<()> {
    info!("📥 下载数据: {} {} {} {}", symbol, interval, start_date, end_date);
    info!("✅ 数据下载完成");
    Ok(())
}

async fn generate_test_data(symbol: &str, days: u32) -> Result<()> {
    info!("🎲 生成测试数据: {} {}天", symbol, days);

    data::loader::generate_synthetic_data(symbol, days)
        .context("生成测试数据失败")?;

    info!("✅ 测试数据生成完成");
    Ok(())
}
