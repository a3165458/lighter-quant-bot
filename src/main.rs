// src/main.rs
mod backtest;
mod dashboard;
mod data;
mod env_profiles;
mod hft;
mod lighter;
mod risk;
mod strategy;
mod utils;

#[cfg(test)]
#[path = "hft_tests.rs"]
mod hft_tests;

use anyhow::{Context, Result};
use chrono::Utc;
use clap::{Parser, Subcommand};
use config::Config;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

#[derive(Parser)]
#[command(author, version, about = "Lighter Trading Bot", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run live trading
    Live {
        /// Config file. When omitted, LIGHTER_NETWORK selects mainnet or Robinhood Chain.
        #[arg(short, long)]
        config: Option<String>,
    },

    /// Run backtest
    Backtest {
        #[arg(short, long)]
        strategy: String,
        #[arg(short, long)]
        data: String,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        #[arg(long, default_value = "10000")]
        capital: f64,
        #[arg(short, long)]
        output: Option<String>,
        /// Strategy params: "grid_count=10,investment=8.0,deviation=0.008"
        #[arg(short, long)]
        params: Option<String>,
        /// Config file for the profitability gate (same section as live)
        #[arg(long)]
        config: Option<String>,
    },

    /// Run parameter optimization sweep
    Optimize {
        #[arg(short, long)]
        strategy: String,
        #[arg(short, long)]
        data: String,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        #[arg(long, default_value = "10000")]
        capital: f64,
        #[arg(short, long)]
        output: Option<String>,
        /// Config file for the profitability gate (same section as live)
        #[arg(long)]
        config: Option<String>,
    },

    /// Start dashboard only
    Dashboard {
        #[arg(long, default_value = "0.0.0.0")]
        host: String,
        #[arg(short, long, default_value = "2028")]
        port: u16,
    },

    /// Download historical data
    Download {
        #[arg(short, long)]
        symbol: String,
        #[arg(short, long, default_value = "1h")]
        interval: String,
        #[arg(long)]
        start: String,
        #[arg(long)]
        end: String,
        /// REST API base URL (mainnet 或 Robinhood Chain 实例)
        #[arg(long, default_value = "https://mainnet.zklighter.elliot.ai")]
        url: String,
        /// 输出文件名标签（默认取 URL 推断: mainnet / rh）
        #[arg(long)]
        tag: Option<String>,
    },

    /// Generate test data
    GenerateData {
        #[arg(short, long, default_value = "BTCUSDT")]
        symbol: String,
        #[arg(short, long, default_value = "30")]
        days: u32,
    },

    /// Observe BBO updates across every discovered market (never places orders)
    Scan {
        #[arg(long, default_value = "https://mainnet.zklighter.elliot.ai")]
        url: String,
        #[arg(long, default_value = "wss://mainnet.zklighter.elliot.ai/stream")]
        ws_url: String,
        #[arg(long, default_value = "30")]
        duration: u64,
        #[arg(long, default_value = "10")]
        top: usize,
        #[arg(long, default_value = "all")]
        market_type: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_profiles::load_shared_env()?;
    utils::logger::init_logger();

    let cli = Cli::parse();

    match cli.command {
        Commands::Live { config } => {
            let config = config.unwrap_or_else(default_live_config_path);
            run_live_trading(&config).await
        }
        Commands::Backtest {
            strategy,
            data,
            start,
            end,
            capital,
            output,
            params,
            config,
        } => {
            run_backtest(
                &strategy,
                &data,
                &start,
                &end,
                capital,
                BacktestCliOpts {
                    output,
                    params,
                    config,
                },
            )
            .await
        }
        Commands::Optimize {
            strategy,
            data,
            start,
            end,
            capital,
            output,
            config,
        } => {
            run_optimize(
                &strategy,
                &data,
                &start,
                &end,
                capital,
                output.as_deref(),
                config.as_deref(),
            )
            .await
        }
        Commands::Dashboard { host, port } => run_dashboard(&host, port).await,
        Commands::Download {
            symbol,
            interval,
            start,
            end,
            url,
            tag,
        } => download_data(&symbol, &interval, &start, &end, &url, tag.as_deref()).await,
        Commands::GenerateData { symbol, days } => generate_test_data(&symbol, days).await,
        Commands::Scan {
            url,
            ws_url,
            duration,
            top,
            market_type,
        } => hft::run_market_scan(&url, &ws_url, duration, top, &market_type).await,
    }
}

fn default_live_config_path() -> String {
    match env_profiles::selected_network().as_str() {
        "robinhood" => "config/settings.robinhood.yaml".to_string(),
        _ => "config/settings.yaml".to_string(),
    }
}

fn universe_mode(settings: &Config) -> String {
    settings
        .get_string("trading.universe.mode")
        .unwrap_or_else(|_| "explicit".to_string())
        .trim()
        .to_ascii_lowercase()
}

fn universe_select_params(settings: &Config) -> hft::UniverseSelectParams {
    hft::UniverseSelectParams {
        max_spread_bps: settings
            .get_float("trading.universe.max_spread_bps")
            .unwrap_or(25.0),
        min_spread_bps: settings
            .get_float("trading.universe.min_spread_bps")
            .unwrap_or(0.0),
        min_size: settings
            .get_float("trading.universe.min_size")
            .unwrap_or(0.01),
        prefer_wider_spreads: settings
            .get_bool("trading.universe.prefer_wider_spreads")
            .unwrap_or(false),
        refresh_interval: std::time::Duration::from_secs(
            settings
                .get_int("trading.universe.refresh_interval_secs")
                .unwrap_or(15)
                .max(1) as u64,
        ),
        actions_per_refresh: settings
            .get_int("trading.universe.actions_per_refresh")
            .unwrap_or(3)
            .max(1) as u32,
    }
}

async fn run_live_trading(config_path: &str) -> Result<()> {
    info!("🚀 Starting Lighter Trading Bot");

    // Load config
    let settings = Config::builder()
        .add_source(config::File::with_name(config_path))
        .add_source(config::Environment::with_prefix("LIGHTER"))
        .build()
        .context("Failed to load config")?;

    let chain_id = settings.get_int("lighter.chain_id").unwrap_or(304);
    let credential_profile = env_profiles::profile_for_chain_id(chain_id)?;
    let (credentials, credential_path) = env_profiles::load_credentials(credential_profile)?;
    info!(
        "🔐 Using {} credential profile from {}",
        credential_profile.network_name(),
        credential_path.display()
    );

    // Load credentials from env
    let secret_key = credentials.secret_key;
    let account_index: i64 = credentials
        .account_index
        .parse()
        .context("Invalid LIGHTER_ACCOUNT_INDEX")?;
    let api_key_index: i32 = credentials
        .api_key_index
        .parse()
        .context("Invalid LIGHTER_API_KEY_INDEX")?;

    let rest_url = settings
        .get_string("lighter.rest_url")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());
    let ws_url = settings
        .get_string("lighter.ws_url")
        .unwrap_or_else(|_| "wss://mainnet.zklighter.elliot.ai/stream".to_string());
    let chain_id = chain_id as i32;

    let max_open_orders = settings.get_int("trading.max_open_orders").unwrap_or(8) as u32;
    info!("⚙️ Max open orders: {}", max_open_orders);

    // Initialize FFI signer (uses API secret key, not L1 private key)
    info!("🔑 Initializing signer...");
    lighter::ffi::init(
        &rest_url,
        &secret_key,
        chain_id,
        api_key_index,
        account_index,
    )
    .context("Failed to initialize FFI signer")?;
    info!(
        "✅ Signer initialized (account={}, api_key_index={})",
        account_index, api_key_index
    );

    // Create REST client (shared between main loop and refresh task)
    let lighter_client = Arc::new(lighter::client::LighterClient::new_with_account(
        &rest_url,
        account_index,
        api_key_index,
    ));

    // Fetch initial nonce
    let nonce = lighter_client
        .refresh_nonce()
        .await
        .context("Failed to fetch nonce")?;
    info!("📋 Initial nonce: {}", nonce);

    // Fetch account info (retry — a crash-loop can 429 the account endpoint)
    info!("📡 Fetching account info...");
    let account = {
        let mut last_err = None;
        let mut fetched = None;
        for attempt in 1..=6 {
            match lighter_client.get_account_info().await {
                Ok(acct) => {
                    fetched = Some(acct);
                    break;
                }
                Err(e) => {
                    warn!("account info attempt {attempt}/6 failed: {e}");
                    last_err = Some(e);
                    tokio::time::sleep(std::time::Duration::from_secs(2 * attempt as u64)).await;
                }
            }
        }
        match fetched {
            Some(acct) => acct,
            None => {
                warn!(
                    "Failed to fetch account info after retries ({}); continuing with an empty snapshot",
                    last_err
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "unknown error".into())
                );
                lighter::account::empty_account_info()
            }
        }
    };
    let equity = account.total_equity;
    let free_balance = account.balances.first().map(|b| b.free).unwrap_or(0.0);
    info!(
        "✅ Account connected — Equity: ${:.2}, Free: ${:.2}, Positions: {}",
        equity,
        free_balance,
        account.positions.len()
    );

    // Cancel all existing orders for a clean start
    info!("🧹 Cancelling all existing orders...");
    match lighter_client.cancel_all_orders("all").await {
        Ok(()) => info!("✅ All existing orders cancelled"),
        Err(e) => warn!("⚠️ Cancel all orders: {} (may have no open orders)", e),
    }
    // Refresh nonce after cancel-all (it consumed one)
    let _ = lighter_client.refresh_nonce().await;

    // Get market configuration
    let markets: Vec<i64> = settings
        .get("trading.markets")
        .unwrap_or_else(|_| vec![0, 1]);
    let configured_market_ids: Vec<u32> = markets.iter().map(|m| *m as u32).collect();

    // Fetch full market list and register symbol map (supports Robinhood Chain instance
    // with stock perps/spot markets — no hardcoded ETH/BTC assumptions)
    let all_markets: Vec<lighter::types::MarketInfo> = match lighter_client.get_all_markets().await
    {
        Ok(ms) => {
            lighter::symbols::register_all(ms.iter().map(|m| (m.market_id, m.symbol.clone())));
            info!(
                "📚 Market registry: {} markets ({} perp)",
                ms.len(),
                ms.iter().filter(|m| m.market_type == "perp").count()
            );
            ms
        }
        Err(e) => {
            warn!(
                "⚠️ Failed to fetch full market list: {} — falling back to ETH/BTC defaults",
                e
            );
            Vec::new()
        }
    };
    let uni_mode = universe_mode(&settings);
    let uni_params = universe_select_params(&settings);
    let live_universe = hft::resolve_live_universe(
        &uni_mode,
        &configured_market_ids,
        &all_markets,
        &[],
        &uni_params,
    );
    if live_universe.awaiting_books {
        info!(
            "🎯 Auto universe: observing {} perp(s); quoting waits for live BBO rank/filter",
            live_universe.subscribe_ids.len()
        );
    } else {
        info!(
            "🎯 Quoting universe: {} market(s) {:?}",
            live_universe.quoting_ids.len(),
            live_universe.quoting_ids
        );
    }
    let observe_ids = live_universe.subscribe_ids.clone();
    let mut quote_ids = live_universe.quoting_ids.clone();
    // Fetch metadata for every observed perp (catalog already has it).
    // Trading is gated on quote_ids / active_markets, which stay empty
    // until live BBO has been ranked.
    let market_ids = if quote_ids.is_empty() {
        observe_ids.clone()
    } else {
        quote_ids.clone()
    };
    lighter_client.set_active_markets(quote_ids.clone());
    // symbol -> MarketInfo for dynamic decimals / min-amount lookups
    let market_registry: std::collections::HashMap<String, lighter::types::MarketInfo> =
        all_markets
            .iter()
            .map(|m| (m.symbol.clone(), m.clone()))
            .collect();

    // Fetch market info (catalog first so auto-universe does not REST-walk every perp)
    let mut market_infos = std::collections::HashMap::new();
    for market in &all_markets {
        market_infos.insert(market.market_id, market.clone());
    }
    for &mid in &quote_ids {
        if market_infos.contains_key(&mid) {
            continue;
        }
        match lighter_client.get_market_info(mid).await {
            Ok(mi) => {
                info!(
                    "📊 Market {}: {} (price_dec={}, size_dec={}, last=${:.2})",
                    mid, mi.symbol, mi.price_decimals, mi.size_decimals, mi.last_trade_price
                );
                market_infos.insert(mid, mi);
            }
            Err(e) => {
                warn!("⚠️ Failed to fetch market {} info: {}", mid, e);
            }
        }
    }

    // Shared open orders counter
    let open_orders_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Setup shared dashboard state
    let network_name = if chain_id == 466324 {
        "robinhood"
    } else {
        "mainnet"
    }
    .to_string();
    let dash_state = Arc::new(RwLock::new(dashboard::server::DashboardState {
        network_name: network_name.clone(),
        rest_url: rest_url.clone(),
        ws_url: ws_url.clone(),
        chain_id,
        equity,
        available_balance: free_balance,
        unrealized_pnl: account.positions.iter().map(|p| p.unrealized_pnl).sum(),
        strategy_name: String::new(),
        total_trades: 0,
        open_orders: 0,
        open_orders_list: Vec::new(),
        positions: account
            .positions
            .iter()
            .map(|p| {
                let mark = if p.size.abs() > 1e-12 {
                    match p.side {
                        lighter::types::Side::Buy => p.entry_price + p.unrealized_pnl / p.size,
                        lighter::types::Side::Sell => p.entry_price - p.unrealized_pnl / p.size,
                    }
                } else {
                    p.entry_price
                };
                serde_json::json!({
                    "symbol": p.symbol,
                    "side": format!("{:?}", p.side),
                    "size": p.size,
                    "entry_price": p.entry_price,
                    "mark_price": mark,
                    "unrealized_pnl": p.unrealized_pnl,
                })
            })
            .collect(),
        trade_history: Vec::new(),
        event_history: Vec::new(),
        risk_status: None,
        daily_realized_pnl: 0.0,
        total_realized_pnl: 0.0,
        initial_equity: equity,
        peak_equity: equity,
        equity_history: vec![(Utc::now().timestamp(), equity)],
        pnl_history: vec![(Utc::now().timestamp(), 0.0)],
        total_volume: 0.0,
        total_closed_trades: 0,
        strategy_params: {
            let mut m = std::collections::HashMap::new();
            m.insert(
                "grid_count".to_string(),
                settings
                    .get_int("trading.strategies.grid_trading.grid_count")
                    .unwrap_or(10)
                    .to_string(),
            );
            m.insert(
                "investment_per_grid".to_string(),
                settings
                    .get_float("trading.strategies.grid_trading.investment_per_grid")
                    .unwrap_or(8.0)
                    .to_string(),
            );
            m.insert(
                "price_deviation".to_string(),
                settings
                    .get_float("trading.strategies.grid_trading.price_deviation")
                    .unwrap_or(0.012)
                    .to_string(),
            );
            m
        },
        strategy_config_changed: false,
        universe_mode: uni_mode.clone(),
        daily_pnl_map: std::collections::HashMap::new(),
        active_markets: quote_ids.clone(),
        trading_paused: false,
        cancel_all_requested: false,
        available_markets: {
            let perps: Vec<(u32, String)> = all_markets
                .iter()
                .filter(|m| m.market_type == "perp")
                .map(|m| (m.market_id, m.symbol.clone()))
                .collect();
            if perps.is_empty() {
                vec![(0, "ETH".to_string()), (1, "BTC".to_string())]
            } else {
                perps
            }
        },
        risk_config: serde_json::json!({
            "max_drawdown_pct": 10.0,
            "daily_loss_limit_pct": 5.0,
            "max_leverage": 5.0,
            "position_stop_loss_pct": 3.0,
            "position_take_profit_pct": 5.0,
            "leverage_limit": 3.0,
        }),
        risk_update_requested: None,
        leverage_limit: 3.0,
        last_prices: std::collections::HashMap::new(),
        quant_agent: dashboard::quant_agent::AgentLedger::load(&network_name),
    }));

    // Restore persistent PnL data from disk
    if let Some(persisted) = dashboard::server::PersistentPnlData::load(&network_name) {
        let mut ds = dash_state.write().await;
        ds.restore_pnl(&persisted);
    }

    // Restore persistent strategy config from disk
    if let Some(saved) = dashboard::server::PersistentStrategyConfig::load(&network_name) {
        let mut ds = dash_state.write().await;
        info!(
            "📂 Loaded strategy config: {} params={:?}",
            saved.strategy_name, saved.strategy_params
        );
        ds.strategy_name = saved.strategy_name;
        ds.strategy_params = saved.strategy_params;
        if !saved.universe_mode.trim().is_empty() {
            ds.universe_mode = saved.universe_mode;
        }
    }

    // Restore persistent risk config from disk
    if let Some(saved) = dashboard::server::PersistentRiskConfig::load(&network_name) {
        let mut ds = dash_state.write().await;
        info!(
            "📂 Loaded risk config: leverage_limit={}",
            saved.leverage_limit
        );
        ds.risk_config = saved.risk_config;
        ds.leverage_limit = saved.leverage_limit;
    }

    // Start dashboard server
    let dash_port = settings.get_int("dashboard.port").unwrap_or(2028) as u16;
    let dash_state_clone = dash_state.clone();
    tokio::spawn(async move {
        if let Err(e) =
            dashboard::server::start_with_state("0.0.0.0", dash_port, dash_state_clone).await
        {
            error!("Dashboard error: {}", e);
        }
    });
    info!("🌐 Dashboard started on port {}", dash_port);

    // Initialize risk manager (shared for periodic equity updates)
    let risk_manager = Arc::new(tokio::sync::Mutex::new(
        risk::risk_manager::RiskManager::new(&settings)
            .context("Failed to initialize risk manager")?,
    ));
    {
        let mut rm = risk_manager.lock().await;
        rm.update_equity(equity);
    }
    // Sync initial risk config from RiskManager to DashboardState
    // If we loaded persistent config, apply it to risk manager
    {
        let ds = dash_state.read().await;
        if ds.leverage_limit != 3.0 {
            // Persistent config was loaded — apply it to risk manager
            let mut rm = risk_manager.lock().await;
            let cfg = &ds.risk_config;
            rm.update_params(
                cfg.get("max_drawdown_pct").and_then(|v| v.as_f64()),
                cfg.get("daily_loss_limit_pct").and_then(|v| v.as_f64()),
                cfg.get("max_leverage").and_then(|v| v.as_f64()),
                cfg.get("position_stop_loss_pct").and_then(|v| v.as_f64()),
                cfg.get("position_take_profit_pct").and_then(|v| v.as_f64()),
            );
        } else {
            // No persistent config — sync risk manager defaults to dashboard
            let rm = risk_manager.lock().await;
            drop(ds);
            let mut ds = dash_state.write().await;
            ds.risk_config = rm.get_config();
            ds.risk_config["leverage_limit"] = serde_json::json!(ds.leverage_limit);
        }
    }

    // Initialize strategy (wrapped in Arc<RwLock> for runtime switching)
    // Use persisted params if available, otherwise fall back to settings.yaml
    let strategy: Arc<tokio::sync::RwLock<Box<dyn strategy::Strategy>>> = {
        let ds = dash_state.read().await;
        let has_saved_params = !ds.strategy_params.is_empty();
        if has_saved_params {
            let params_str = ds
                .strategy_params
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(",");
            info!(
                "📂 Creating strategy from saved config: {} params={}",
                ds.strategy_name, params_str
            );
            let strat = strategy::create_strategy_with_params(&ds.strategy_name, Some(&params_str))
                .unwrap_or_else(|e| {
                    warn!(
                        "Failed to create strategy from saved params: {}, falling back to defaults",
                        e
                    );
                    strategy::create_strategy(&settings).expect("Failed to create default strategy")
                });
            Arc::new(tokio::sync::RwLock::new(strat))
        } else {
            Arc::new(tokio::sync::RwLock::new(
                strategy::create_strategy(&settings).context("Failed to initialize strategy")?,
            ))
        }
    };
    let strategy_name = strategy.read().await.name().to_string();
    info!("📈 Strategy: {}", strategy_name);

    // Update dashboard with strategy name
    {
        let mut ds = dash_state.write().await;
        ds.strategy_name = strategy_name.clone();
    }

    // Create data store
    let data_store = Arc::new(RwLock::new(data::storage::MarketDataStore::new()));

    // Fetch initial candle data for strategies that need history.
    // Auto universe waits for BBO ranking before it has a quoting set.
    for &mid in &quote_ids {
        let symbol = market_infos
            .get(&mid)
            .map(|m| m.symbol.as_str())
            .unwrap_or("UNKNOWN");
        match lighter_client.get_candlesticks(mid, "1h", 100).await {
            Ok(candles) => {
                info!("📊 Loaded {} candles for {}", candles.len(), symbol);
                let mut store = data_store.write().await;
                for candle in candles {
                    store.add_candle(candle);
                }
            }
            Err(e) => {
                warn!("⚠️ Failed to fetch candles for {}: {}", symbol, e);
            }
        }
    }

    // Flag to pause order placement during auto-reset (prevents nonce race)
    let grid_resetting = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let latest_bbos: Arc<std::sync::Mutex<std::collections::HashMap<u32, hft::BboUpdate>>> =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    // Connect WebSocket
    info!("🔌 Connecting WebSocket...");
    let ws_client = lighter::websocket::LighterWebSocket::new(&ws_url);
    ws_client.connect().await?;

    // Auto universe: ticker-subscribe every discovered perp (sharded) so BBO
    // can rank the quoting set. Explicit mode keeps full market-data subs.
    let mut subscribed = std::collections::HashSet::new();
    let auto_universe = uni_mode == "auto" || uni_mode == "all";
    if auto_universe {
        let shards = hft::plan_subscription_shards(&observe_ids, 100)?;
        for (shard_index, shard) in shards.iter().enumerate() {
            if shard_index == 0 {
                for &mid in shard {
                    ws_client.subscribe_ticker(mid).await?;
                    subscribed.insert(mid);
                    tokio::time::sleep(std::time::Duration::from_millis(310)).await;
                }
            } else {
                let shard = shard.clone();
                let store = data_store.clone();
                let bbos = latest_bbos.clone();
                let url = ws_url.clone();
                tokio::spawn(async move {
                    let conn = lighter::websocket::LighterWebSocket::new(&url);
                    if let Err(error) = conn.connect().await {
                        warn!(shard = shard_index, %error, "Auto-universe ticker shard failed to connect");
                        return;
                    }
                    let mut receiver = conn.get_receiver();
                    for mid in &shard {
                        if let Err(error) = conn.subscribe_ticker(*mid).await {
                            warn!(shard = shard_index, market = mid, %error, "Ticker subscribe failed");
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(310)).await;
                    }
                    while let Ok(message) = receiver.recv().await {
                        if let lighter::types::WsMessage::BboUpdate(bbo) = message {
                            store
                                .write()
                                .await
                                .update_order_book(hft::order_book_from_bbo(&bbo));
                            bbos.lock().unwrap().insert(bbo.market_id, bbo);
                        }
                    }
                });
            }
        }
        info!(
            "📡 Auto universe: ticker-subscribed {} perp(s) for BBO ranking",
            subscribed.len()
        );
    } else {
        for &mid in &market_ids {
            let symbol = market_infos
                .get(&mid)
                .map(|m| m.symbol.as_str())
                .unwrap_or("?");
            ws_client.subscribe_market_data(&mid.to_string()).await?;
            subscribed.insert(mid);
            info!("📡 Subscribed to {} (market {})", symbol, mid);
        }
    }

    // Start the main trading loop
    let mut ws_receiver = ws_client.get_receiver();
    let data_store_clone = data_store.clone();

    // Periodic account refresh task — updates dashboard, risk manager
    // Also checks for emergency close and per-position stop-loss/take-profit
    // Share the client with the refresh task (same nonce counter)
    let client_for_refresh = lighter_client.clone();
    let dash_state_refresh = dash_state.clone();
    let risk_manager_refresh = risk_manager.clone();
    let open_orders_refresh = open_orders_count.clone();
    let market_infos_refresh = market_infos.clone();
    let strategy_refresh = strategy.clone();
    let grid_resetting_refresh = grid_resetting.clone();
    let stale_price_pct = 0.012_f64; // Cancel orders >1.2% from mid price
    let max_order_age_secs = 300_u64; // Force cancel-all after 5 minutes if stale
    let resolver_configured_ids = configured_market_ids.clone();
    let configured_market_ids = market_ids.clone();
    let registry_refresh = market_registry.clone();
    tokio::spawn(async move {
        // 动态市场元数据查询（回退到主网 ETH/BTC 默认值）
        let market_id_of = |sym: &str| -> Option<u32> {
            registry_refresh
                .get(sym)
                .map(|m| m.market_id)
                .or_else(|| lighter::symbols::market_id_of(sym))
        };
        let size_decimals_of = |sym: &str| -> i32 {
            registry_refresh
                .get(sym)
                .map(|m| m.size_decimals as i32)
                .unwrap_or(if sym == "BTC" { 5 } else { 4 })
        };
        let min_change_of = |sym: &str| -> f64 {
            registry_refresh
                .get(sym)
                .map(|m| m.min_base_amount * 0.95)
                .unwrap_or(if sym == "BTC" { 0.00019 } else { 0.0049 })
        };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        let mut at_max_since: Option<std::time::Instant> = None;
        // Equity-based realized PnL tracking
        let mut prev_equity: f64 = 0.0;
        let mut prev_unrealized: f64 = 0.0;
        // Position snapshot for logging close events (side, size, entry_price)
        let mut prev_positions: std::collections::HashMap<
            String,
            (lighter::types::Side, f64, f64),
        > = std::collections::HashMap::new();
        let mut position_opened_at: std::collections::HashMap<String, chrono::DateTime<Utc>> =
            std::collections::HashMap::new();
        // Track daily PnL reset
        let mut last_daily_reset_day: u32 = (Utc::now().timestamp() / 86400) as u32;
        let mut first_cycle = true;
        let mut empty_open_order_streak = 0u32;
        loop {
            interval.tick().await;

            // Always sync real open orders count first (fast, lightweight)
            match client_for_refresh.get_open_orders("all").await {
                Ok(orders) => {
                    let mut orders = orders;
                    let mut count = orders.len() as u32;
                    let prev = open_orders_refresh.load(std::sync::atomic::Ordering::Relaxed);
                    // A single empty accountActiveOrders page can be truncated.
                    // Confirm immediately, then reconcile local ghosts to the
                    // exchange instead of ignoring 0 forever.
                    let mut streak = empty_open_order_streak;
                    if count == 0 && prev > 0 {
                        match client_for_refresh.get_open_orders("all").await {
                            Ok(retry) => {
                                orders = retry;
                                count = orders.len() as u32;
                                if count == 0 {
                                    streak = streak.max(1);
                                }
                            }
                            Err(e) => {
                                warn!("Open-order confirm fetch failed: {e}");
                            }
                        }
                    }
                    let decision =
                        risk::order_sync::reconcile_open_order_count(count, prev, streak);
                    empty_open_order_streak = decision.consecutive_empty;
                    match decision.action {
                        risk::order_sync::OpenOrderReconcileAction::HoldPendingConfirm => {
                            warn!(
                                prev,
                                "Open-order sync 0 vs local {prev}; confirming before reconcile"
                            );
                        }
                        risk::order_sync::OpenOrderReconcileAction::ReconcileToExchange => {
                            warn!(
                                prev,
                                "Reconciling open-order count to exchange 0 (was local {prev})"
                            );
                            strategy_refresh.read().await.clear_filled_state();
                        }
                        risk::order_sync::OpenOrderReconcileAction::TrustExchange => {}
                    }
                    let trusted = decision.trusted_count;
                    open_orders_refresh.store(trusted, std::sync::atomic::Ordering::Relaxed);
                    if prev != trusted {
                        info!("📋 Open orders synced: {} → {} (real)", prev, trusted);
                    }
                    let mut ds = dash_state_refresh.write().await;
                    ds.open_orders = count;
                    // Also store the actual orders for dashboard display
                    ds.open_orders_list = orders
                        .iter()
                        .map(|o| {
                            serde_json::json!({
                                "id": o.id,
                                "symbol": o.symbol,
                                "side": format!("{:?}", o.side),
                                "price": o.price,
                                "quantity": o.quantity,
                                "filled_quantity": o.filled_quantity,
                                "status": format!("{:?}", o.status),
                            })
                        })
                        .collect();
                    drop(ds);

                    // ===== Stale order management =====
                    // SKIP during emergency — don't cancel close orders
                    let is_emergency = {
                        let rm = risk_manager_refresh.lock().await;
                        rm.is_emergency_triggered()
                    };
                    if count > 0 && !is_emergency {
                        // Get fresh market prices
                        let mut mid_prices: std::collections::HashMap<u32, f64> =
                            std::collections::HashMap::new();
                        for &mid in &configured_market_ids {
                            if let Ok(mi) = client_for_refresh.get_market_info(mid).await {
                                if mi.last_trade_price > 0.0 {
                                    mid_prices.insert(mid, mi.last_trade_price);
                                }
                            }
                        }

                        // Signed net position per market (long = +, short = -) from last
                        // account refresh. Used to spare position-reducing orders from the
                        // stale-cancel sweeps below — otherwise the very orders that would
                        // flatten an accumulated bag get wiped before they can fill.
                        let net_pos_by_market: std::collections::HashMap<u32, f64> = {
                            let ds = dash_state_refresh.read().await;
                            let mut m = std::collections::HashMap::new();
                            for p in &ds.positions {
                                if let (Some(sym), Some(size), Some(side)) =
                                    (p["symbol"].as_str(), p["size"].as_f64(), p["side"].as_str())
                                {
                                    if let Some(mid) = market_id_of(sym) {
                                        let signed = if side == "Sell" {
                                            -size.abs()
                                        } else {
                                            size.abs()
                                        };
                                        *m.entry(mid).or_insert(0.0) += signed;
                                    }
                                }
                            }
                            m
                        };
                        // An order reduces the position if it trades against the net side.
                        let reduces_position =
                            |market_id: u32, side: &lighter::types::Side| -> bool {
                                match net_pos_by_market.get(&market_id).copied().unwrap_or(0.0) {
                                    n if n > 1e-9 => matches!(side, lighter::types::Side::Sell),
                                    n if n < -1e-9 => matches!(side, lighter::types::Side::Buy),
                                    _ => false,
                                }
                            };

                        // 诊断：打印每个挂单相对市价的距离（R = 减仓单）。
                        // 2026-07-24 的 15.5h 停摆就是靠这类信息才能判定挂单是"太远不成交"还是"够近但被占位"。
                        {
                            let summary: Vec<String> = orders
                                .iter()
                                .filter_map(|o| {
                                    let mid = *mid_prices.get(&market_id_of(&o.symbol)?)?;
                                    let tag = if reduces_position(market_id_of(&o.symbol)?, &o.side)
                                    {
                                        "R"
                                    } else {
                                        ""
                                    };
                                    Some(format!(
                                        "{:?}{}@{:.1}({:+.2}%)",
                                        o.side,
                                        tag,
                                        o.price,
                                        (o.price - mid) / mid * 100.0
                                    ))
                                })
                                .collect();
                            if !summary.is_empty() {
                                info!("📋 Orders vs mid: {}", summary.join(" "));
                            }
                        }

                        // 逃生阀（修复 2026-07-24 死锁）：减仓单可以豁免撤单，但**绝不允许占满所有槽位**。
                        // 否则「持仓封顶 → 只出单一方向信号 → 全部挂单都是减仓单 → 全部豁免 → 槽位占死
                        // → 新信号被 max_open_orders 拒绝」构成无出口的闭环，机器人完全停摆。
                        // 只保护最接近市价（最可能成交）的 N 个减仓单，至少留 2 个槽位给新信号。
                        let reducing_keep_limit = max_open_orders.saturating_sub(2) as usize;
                        let protected_reducing: std::collections::HashSet<String> = {
                            let mut cand: Vec<(&str, f64)> = orders
                                .iter()
                                .filter_map(|o| {
                                    let mid_id = market_id_of(&o.symbol)?;
                                    if !reduces_position(mid_id, &o.side) {
                                        return None;
                                    }
                                    let mid = *mid_prices.get(&mid_id)?;
                                    Some((o.id.as_str(), (o.price - mid).abs() / mid))
                                })
                                .collect();
                            cand.sort_by(|a, b| {
                                a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                            });
                            cand.truncate(reducing_keep_limit);
                            cand.into_iter().map(|(id, _)| id.to_string()).collect()
                        };

                        // Strategy 1: Cancel individual orders that are too far from mid price
                        let mut cancelled = 0u32;
                        for order in &orders {
                            let market_id = match market_id_of(&order.symbol) {
                                Some(id) => id,
                                None => continue,
                            };
                            // 减仓单原则上保留（让它成交去平仓），但仅限受逃生阀保护的那几个。
                            // 未受保护的（离市价最远的）即使是减仓单也照常按 >1.2% 撤掉：
                            // 它短期内不会成交，占着槽位纯属死重；撤掉后策略会在更贴近市价的
                            // 网格层重新挂减仓单，反而**加快**平仓。
                            if reduces_position(market_id, &order.side)
                                && protected_reducing.contains(&order.id)
                            {
                                continue;
                            }
                            if let Some(&mid) = mid_prices.get(&market_id) {
                                let diff = (order.price - mid).abs() / mid;
                                if diff > stale_price_pct {
                                    if let Ok(idx) = order.id.parse::<i64>() {
                                        info!("🗑️ Cancelling stale order: {} {:?} @ {:.2} (mid={:.2}, diff={:.1}%)",
                                            order.symbol, order.side, order.price, mid, diff * 100.0);
                                        match client_for_refresh
                                            .cancel_order_by_index(market_id, idx)
                                            .await
                                        {
                                            Ok(()) => cancelled += 1,
                                            Err(e) => warn!(
                                                "Failed to cancel stale order {}: {}",
                                                order.id, e
                                            ),
                                        }
                                    }
                                }
                            }
                        }

                        if cancelled > 0 {
                            info!(
                                "🗑️ Cancelled {} stale orders, resetting grid state",
                                cancelled
                            );
                            strategy_refresh.read().await.clear_filled_state();
                            let _ = client_for_refresh.refresh_nonce().await;
                            let new_count = count.saturating_sub(cancelled);
                            open_orders_refresh
                                .store(new_count, std::sync::atomic::Ordering::Relaxed);
                            at_max_since = None; // Reset timer
                        }

                        // Strategy 2: Time-based cancel-all if orders sit unfilled too long
                        // This prevents the bot from being stuck when orders are within
                        // the grid range but none are filling
                        if count >= 3 {
                            // Any meaningful number of open orders
                            if at_max_since.is_none() {
                                at_max_since = Some(std::time::Instant::now());
                            }
                            if let Some(since) = at_max_since {
                                let elapsed = since.elapsed().as_secs();
                                if elapsed >= max_order_age_secs {
                                    // Cancel only the position-INCREASING orders; keep any
                                    // reducing orders alive so the accumulated position can
                                    // still be closed when price returns to those levels.
                                    grid_resetting_refresh
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                    let mut auto_cancelled = 0u32;
                                    let mut kept_reducing = 0u32;
                                    for order in &orders {
                                        let market_id = match market_id_of(&order.symbol) {
                                            Some(id) => id,
                                            None => continue,
                                        };
                                        // 同逃生阀：只保护最接近市价的若干减仓单，其余照撤，
                                        // 保证永远有空槽位接纳新信号。
                                        if reduces_position(market_id, &order.side)
                                            && protected_reducing.contains(&order.id)
                                        {
                                            kept_reducing += 1;
                                            continue;
                                        }
                                        if let Ok(idx) = order.id.parse::<i64>() {
                                            match client_for_refresh
                                                .cancel_order_by_index(market_id, idx)
                                                .await
                                            {
                                                Ok(()) => auto_cancelled += 1,
                                                Err(e) => warn!(
                                                    "❌ Auto-reset cancel failed for {}: {}",
                                                    order.id, e
                                                ),
                                            }
                                        }
                                    }
                                    info!("🔄 Auto-reset: cancelled {} stale orders, kept {} reducing (stale for {}s)",
                                        auto_cancelled, kept_reducing, elapsed);
                                    if auto_cancelled > 0 {
                                        strategy_refresh.read().await.clear_filled_state();
                                        let _ = client_for_refresh.refresh_nonce().await;
                                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                    }
                                    let remaining = count.saturating_sub(auto_cancelled);
                                    open_orders_refresh
                                        .store(remaining, std::sync::atomic::Ordering::Relaxed);
                                    at_max_since = None;
                                    grid_resetting_refresh
                                        .store(false, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                        } else {
                            at_max_since = None; // Below threshold, reset timer
                        }
                    } else {
                        at_max_since = None;
                    }
                }
                Err(e) => {
                    warn!("⚠️ Open orders sync error: {}", e);
                }
            }

            // Refresh account info (every cycle)
            match client_for_refresh.get_account_info().await {
                Ok(acct) => {
                    let curr_equity = acct.total_equity;
                    let curr_unrealized: f64 =
                        acct.positions.iter().map(|p| p.unrealized_pnl).sum();

                    // ===== Auto-close positions on non-configured markets =====
                    for pos in &acct.positions {
                        if pos.size.abs() < 1e-10 {
                            continue;
                        }
                        let pos_market_id = match market_id_of(&pos.symbol) {
                            Some(id) => id,
                            None => {
                                warn!("⚠️ Unknown market for position symbol {}, skipping auto-close check", pos.symbol);
                                continue;
                            }
                        };
                        if !configured_market_ids.contains(&pos_market_id) {
                            warn!("⚠️ Found position on non-configured market {}: {} {:?} {:.6} — closing",
                                pos.symbol, pos.symbol, pos.side, pos.size);
                            let close_side = match pos.side {
                                lighter::types::Side::Buy => lighter::types::Side::Sell,
                                lighter::types::Side::Sell => lighter::types::Side::Buy,
                            };
                            let mi = market_infos_refresh.values().find(|m| {
                                pos.symbol.contains(&m.symbol) || m.symbol.contains(&pos.symbol)
                            });
                            // Get fresh price for this market
                            let fresh_price =
                                match client_for_refresh.get_market_info(pos_market_id).await {
                                    Ok(fmi) if fmi.last_trade_price > 0.0 => fmi.last_trade_price,
                                    _ => pos.entry_price,
                                };
                            let slippage = 0.005;
                            let close_price = match close_side {
                                lighter::types::Side::Buy => fresh_price * (1.0 + slippage),
                                lighter::types::Side::Sell => fresh_price * (1.0 - slippage),
                            };
                            warn!(
                                "🔄 Auto-closing {} {:?} {:.6} @ {:.2} (non-configured market)",
                                pos.symbol, close_side, pos.size, close_price
                            );
                            match client_for_refresh
                                .place_order_with_market(
                                    pos_market_id,
                                    close_side,
                                    close_price,
                                    pos.size.abs(),
                                    mi,
                                    true,
                                )
                                .await
                            {
                                Ok(resp) => info!(
                                    "✅ Auto-close order placed: {} id={}",
                                    pos.symbol, resp.order_id
                                ),
                                Err(e) => error!("❌ Auto-close failed for {}: {}", pos.symbol, e),
                            }
                        }
                    }

                    // ===== Realized PnL detection =====
                    // Step 1: Detect actual position changes (size or side changed)
                    // Step 2: Only then compute PnL via equity method: realized = Δequity - Δunrealized
                    let mut realized_pnl_this_cycle = 0.0_f64;
                    let mut close_events: Vec<serde_json::Value> = Vec::new();
                    let close_timestamp = Utc::now();
                    let mut curr_pos_map: std::collections::HashMap<
                        String,
                        (lighter::types::Side, f64, f64),
                    > = std::collections::HashMap::new();
                    for p in &acct.positions {
                        if p.size.abs() > 1e-10 {
                            let factor = 10_f64.powi(size_decimals_of(&p.symbol));
                            let rounded_size = (p.size * factor).round() / factor;
                            curr_pos_map
                                .insert(p.symbol.clone(), (p.side, rounded_size, p.entry_price));
                        }
                    }

                    if !first_cycle && prev_equity > 0.0 {
                        // Check for meaningful position changes (size decreased or position closed)
                        let mut position_reductions: Vec<(
                            String,
                            lighter::types::Side,
                            f64,
                            f64,
                            &str,
                        )> = Vec::new();
                        for (symbol, (prev_side, prev_size, prev_entry)) in &prev_positions {
                            // Min change threshold per market
                            let min_change = min_change_of(symbol);
                            match curr_pos_map.get(symbol) {
                                Some((curr_side, curr_size, _)) => {
                                    if *curr_side != *prev_side {
                                        // Side flipped — full close of old position
                                        position_reductions.push((
                                            symbol.clone(),
                                            *prev_side,
                                            *prev_size,
                                            *prev_entry,
                                            "Full Close",
                                        ));
                                    } else if *prev_size - *curr_size >= min_change {
                                        // Position reduced
                                        let closed = prev_size - curr_size;
                                        let close_type = if *curr_size < min_change {
                                            "Full Close"
                                        } else {
                                            "Partial Close"
                                        };
                                        position_reductions.push((
                                            symbol.clone(),
                                            *prev_side,
                                            closed,
                                            *prev_entry,
                                            close_type,
                                        ));
                                    }
                                }
                                None => {
                                    // Position gone entirely
                                    if *prev_size >= 1e-10 {
                                        position_reductions.push((
                                            symbol.clone(),
                                            *prev_side,
                                            *prev_size,
                                            *prev_entry,
                                            "Full Close",
                                        ));
                                    }
                                }
                            }
                        }

                        // Only compute PnL if an actual position change occurred
                        if !position_reductions.is_empty() {
                            let equity_change = curr_equity - prev_equity;
                            let unrealized_change = curr_unrealized - prev_unrealized;
                            let closed_notional: f64 = position_reductions
                                .iter()
                                .map(|(_, _, size, entry, _)| size * entry)
                                .sum();
                            let all_vanished =
                                curr_pos_map.is_empty() && !prev_positions.is_empty();
                            match dashboard::pnl_accounting::realized_from_position_reductions(
                                equity_change,
                                unrealized_change,
                                closed_notional,
                                all_vanished,
                            ) {
                                Some(realized) => realized_pnl_this_cycle = realized,
                                None => {
                                    warn!(
                                        equity_change,
                                        unrealized_change,
                                        closed_notional,
                                        all_vanished,
                                        "⏭️ Skipping realized PnL — account snapshot looks incomplete"
                                    );
                                    position_reductions.clear();
                                }
                            }

                            // Distribute PnL across changed positions proportionally
                            let total_notional: f64 = position_reductions
                                .iter()
                                .map(|(_, _, size, entry, _)| size * entry)
                                .sum();

                            for (symbol, prev_side, closed_size, prev_entry, close_type) in
                                &position_reductions
                            {
                                let pnl_share =
                                    if total_notional > 0.0 && position_reductions.len() > 1 {
                                        realized_pnl_this_cycle * (closed_size * prev_entry)
                                            / total_notional
                                    } else {
                                        realized_pnl_this_cycle
                                    };
                                let market_id = market_id_of(symbol).unwrap_or(0);

                                info!(
                                    "💰 {} {}: {:?} {:.6} @ entry={:.2} | PnL: {}{:.4}",
                                    close_type,
                                    symbol,
                                    prev_side,
                                    closed_size,
                                    prev_entry,
                                    if pnl_share >= 0.0 { "+" } else { "" },
                                    pnl_share
                                );

                                let duration_secs = position_opened_at
                                    .get(symbol)
                                    .map(|opened| {
                                        close_timestamp
                                            .signed_duration_since(*opened)
                                            .num_seconds()
                                            .max(0)
                                    })
                                    .unwrap_or(0);

                                close_events.push(serde_json::json!({
                                    "timestamp": close_timestamp.to_rfc3339(),
                                    "symbol": symbol,
                                    "market_id": market_id,
                                    "side": format!("{:?}", match prev_side {
                                        lighter::types::Side::Buy => lighter::types::Side::Sell,
                                        lighter::types::Side::Sell => lighter::types::Side::Buy,
                                    }),
                                    "price": prev_entry,
                                    "quantity": closed_size,
                                    "pnl": (pnl_share * 10000.0).round() / 10000.0,
                                    "action": close_type, // "Full Close" or "Partial Close"
                                    "duration_secs": duration_secs,
                                }));
                            }
                        }
                    }
                    first_cycle = false;
                    prev_equity = curr_equity;
                    prev_unrealized = curr_unrealized;

                    let mut next_opened_at = position_opened_at.clone();
                    next_opened_at.retain(|symbol, _| curr_pos_map.contains_key(symbol));
                    for (symbol, (curr_side, curr_size, _)) in &curr_pos_map {
                        let min_change = min_change_of(symbol);
                        let should_reset = match prev_positions.get(symbol) {
                            Some((prev_side, prev_size, _)) => {
                                *prev_side != *curr_side || *prev_size < min_change
                            }
                            None => *curr_size >= min_change,
                        };
                        if should_reset || !next_opened_at.contains_key(symbol) {
                            next_opened_at.insert(symbol.clone(), close_timestamp);
                        }
                    }
                    position_opened_at = next_opened_at;

                    // Update position snapshot (rounded) for next cycle
                    prev_positions.clear();
                    for p in &acct.positions {
                        if p.size.abs() > 1e-10 {
                            let factor = 10_f64.powi(size_decimals_of(&p.symbol));
                            let rounded_size = (p.size * factor).round() / factor;
                            prev_positions
                                .insert(p.symbol.clone(), (p.side, rounded_size, p.entry_price));
                        }
                    }

                    // Update dashboard
                    {
                        let mut ds = dash_state_refresh.write().await;
                        ds.equity = curr_equity;
                        ds.available_balance = acct.balances.first().map(|b| b.free).unwrap_or(0.0);
                        ds.unrealized_pnl = curr_unrealized;
                        ds.positions = acct.positions.iter().map(|p| {
                            // Calculate mark price from unrealized PnL
                            let mark = if p.size.abs() > 1e-12 {
                                match p.side {
                                    lighter::types::Side::Buy => p.entry_price + p.unrealized_pnl / p.size,
                                    lighter::types::Side::Sell => p.entry_price - p.unrealized_pnl / p.size,
                                }
                            } else { p.entry_price };
                            serde_json::json!({
                                "symbol": p.symbol,
                                "side": format!("{:?}", p.side),
                                "size": p.size,
                                "entry_price": p.entry_price,
                                "mark_price": mark,
                                "unrealized_pnl": p.unrealized_pnl,
                                "opened_at": position_opened_at.get(&p.symbol).map(|ts| ts.to_rfc3339()),
                            })
                        }).collect();

                        // ===== Update realized PnL =====
                        if realized_pnl_this_cycle.abs() > 0.0001 {
                            // Daily reset check
                            let today = (Utc::now().timestamp() / 86400) as u32;
                            if today > last_daily_reset_day {
                                // Save yesterday's daily PnL to map before reset
                                let yesterday = chrono::Utc::now()
                                    .checked_sub_signed(chrono::Duration::days(1))
                                    .map(|d| d.format("%Y-%m-%d").to_string())
                                    .unwrap_or_default();
                                if !yesterday.is_empty() && ds.daily_realized_pnl.abs() > 0.0001 {
                                    let yesterday_pnl = ds.daily_realized_pnl;
                                    ds.daily_pnl_map.insert(yesterday, yesterday_pnl);
                                }
                                info!(
                                    "📅 New day — resetting daily realized PnL ({:.4} → 0.0)",
                                    ds.daily_realized_pnl
                                );
                                ds.daily_realized_pnl = 0.0;
                                last_daily_reset_day = today;
                            }
                            ds.daily_realized_pnl += realized_pnl_this_cycle;
                            ds.total_realized_pnl += realized_pnl_this_cycle;
                            info!(
                                "📊 Realized PnL update: cycle={:+.4}, daily={:+.4}, total={:+.4}",
                                realized_pnl_this_cycle,
                                ds.daily_realized_pnl,
                                ds.total_realized_pnl
                            );
                        }

                        // Record close events in trade history (lifetime volume/close
                        // counters + shared ring-buffer limit live in push_trade).
                        let has_close_events = !close_events.is_empty();
                        for evt in close_events {
                            ds.push_trade(evt);
                        }

                        if realized_pnl_this_cycle.abs() > 0.0001 || has_close_events {
                            ds.save_pnl();
                        }

                        // Track initial equity on first update
                        if ds.initial_equity == 0.0 {
                            ds.initial_equity = curr_equity;
                        }
                        // Rebase lifetime realized when it has drifted from equity.
                        // 2026-08-14: a vanished-position snapshot booked +$329 realized
                        // while cash equity only moved a few dollars.
                        let implied = dashboard::pnl_accounting::implied_realized(
                            curr_equity,
                            ds.initial_equity,
                            curr_unrealized,
                        );
                        if dashboard::pnl_accounting::realized_needs_rebase(
                            ds.total_realized_pnl,
                            implied,
                        ) {
                            let delta = implied - ds.total_realized_pnl;
                            warn!(
                                stored = ds.total_realized_pnl,
                                implied, delta, "Reconciling realized PnL to equity identity"
                            );
                            ds.total_realized_pnl = implied;
                            // A $100+ identity jump is almost always a snapshot
                            // glitch (2026-08-14 +$329, 2026-08-15 -$201). Do
                            // not dump it into today's loss and trip emergency.
                            if delta.abs() <= 20.0 {
                                ds.daily_realized_pnl += delta;
                            }
                            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                            let daily_now = ds.daily_realized_pnl;
                            ds.daily_pnl_map.insert(today, daily_now);
                            ds.save_pnl();
                        }
                        // Track peak equity
                        if curr_equity > ds.peak_equity {
                            ds.peak_equity = curr_equity;
                        }
                        // Record equity history (max 1440 points = 24h at 1/min)
                        let now_ts = Utc::now().timestamp();
                        let should_record = ds
                            .equity_history
                            .last()
                            .map(|(ts, _)| now_ts - ts >= 60)
                            .unwrap_or(true);
                        if should_record {
                            ds.equity_history.push((now_ts, curr_equity));
                            let cum_pnl = curr_equity - ds.initial_equity;
                            ds.pnl_history.push((now_ts, cum_pnl));
                            // Keep up to 10080 points (~7 days at 1/min)
                            if ds.equity_history.len() > 10080 {
                                ds.equity_history.remove(0);
                                ds.pnl_history.remove(0);
                            }
                            // Periodic save: every 5 minutes
                            let should_periodic_save = ds.equity_history.len() % 5 == 0;
                            if should_periodic_save {
                                ds.save_pnl();
                            }
                        }
                    }

                    // Update risk manager equity
                    let daily_pnl: f64 = acct.positions.iter().map(|p| p.unrealized_pnl).sum();
                    let is_emergency = {
                        let mut rm = risk_manager_refresh.lock().await;
                        rm.update_equity(acct.total_equity);
                        rm.update_daily_pnl(daily_pnl);

                        // Check if emergency close should trigger
                        if rm.should_emergency_close() && !rm.is_emergency_triggered() {
                            warn!("🚨 紧急平仓触发! 取消所有订单并平仓...");
                            rm.set_emergency_triggered();
                        }
                        rm.is_emergency_triggered()
                    }; // rm lock released here

                    // ===== Emergency close: keep retrying until flat =====
                    if is_emergency {
                        let has_positions = acct.positions.iter().any(|p| p.size.abs() > 1e-10);
                        if has_positions {
                            // Cancel all orders to free margin
                            let _ = client_for_refresh.cancel_all_orders("all").await;
                            let _ = client_for_refresh.refresh_nonce().await;

                            // Get fresh market prices for aggressive close
                            let mut fresh_prices: std::collections::HashMap<u32, f64> =
                                std::collections::HashMap::new();
                            // Fetch prices for configured markets plus any market with an open position
                            let mut close_mids: std::collections::HashSet<u32> =
                                configured_market_ids.iter().copied().collect();
                            for pos in &acct.positions {
                                if pos.size.abs() < 1e-10 {
                                    continue;
                                }
                                if let Some(id) = market_id_of(&pos.symbol) {
                                    close_mids.insert(id);
                                }
                            }
                            for &mid in &close_mids {
                                if let Ok(fmi) = client_for_refresh.get_market_info(mid).await {
                                    if fmi.last_trade_price > 0.0 {
                                        fresh_prices.insert(mid, fmi.last_trade_price);
                                    }
                                }
                            }

                            // Close all positions at CURRENT market price with slippage
                            for pos in &acct.positions {
                                if pos.size.abs() < 1e-10 {
                                    continue;
                                }
                                let close_side = match pos.side {
                                    lighter::types::Side::Buy => lighter::types::Side::Sell,
                                    lighter::types::Side::Sell => lighter::types::Side::Buy,
                                };
                                let mi = market_infos_refresh.values().find(|m| {
                                    pos.symbol.contains(&m.symbol)
                                        || m.symbol.contains(&pos.symbol.replace("market_", ""))
                                });
                                let market_id = mi
                                    .map(|m| m.market_id)
                                    .or_else(|| market_id_of(&pos.symbol))
                                    .unwrap_or(0);

                                // Use CURRENT market price + slippage (not entry price!)
                                let current_mid = fresh_prices
                                    .get(&market_id)
                                    .copied()
                                    .unwrap_or(pos.entry_price);
                                let slippage = 0.005; // 0.5% slippage for aggressive fill
                                let close_price = match close_side {
                                    lighter::types::Side::Buy => current_mid * (1.0 + slippage),
                                    lighter::types::Side::Sell => current_mid * (1.0 - slippage),
                                };

                                warn!(
                                    "🚨 紧急平仓: {} {:?} {:.6} @ {:.2} (mid={:.2}, entry={:.2})",
                                    pos.symbol,
                                    close_side,
                                    pos.size,
                                    close_price,
                                    current_mid,
                                    pos.entry_price
                                );
                                match client_for_refresh
                                    .place_order_with_market(
                                        market_id,
                                        close_side,
                                        close_price,
                                        pos.size.abs(),
                                        mi,
                                        true,
                                    )
                                    .await
                                {
                                    Ok(resp) => info!("✅ 紧急平仓订单: id={}", resp.order_id),
                                    Err(e) => error!("❌ 紧急平仓订单失败: {}", e),
                                }
                            }
                            continue; // skip normal processing this cycle
                        } else {
                            info!("✅ 紧急平仓完成 — 所有持仓已关闭");
                        }
                    }

                    // ===== Per-position stop-loss / take-profit check =====
                    if !is_emergency {
                        // Build current prices from last known market data
                        let mut current_prices = std::collections::HashMap::new();
                        for mi in market_infos_refresh.values() {
                            if mi.last_trade_price > 0.0 {
                                current_prices.insert(mi.symbol.clone(), mi.last_trade_price);
                                current_prices.insert(
                                    format!("market_{}", mi.market_id),
                                    mi.last_trade_price,
                                );
                            }
                        }

                        for (&mid, mi) in &market_infos_refresh {
                            if let Ok(fresh_mi) = client_for_refresh.get_market_info(mid).await {
                                current_prices
                                    .insert(fresh_mi.symbol.clone(), fresh_mi.last_trade_price);
                                current_prices
                                    .insert(format!("market_{}", mid), fresh_mi.last_trade_price);
                            } else {
                                current_prices.insert(mi.symbol.clone(), mi.last_trade_price);
                                current_prices
                                    .insert(format!("market_{}", mid), mi.last_trade_price);
                            }
                        }

                        let close_signals = {
                            let rm = risk_manager_refresh.lock().await;
                            rm.check_position_stop_loss_take_profit(
                                &acct.positions,
                                &current_prices,
                            )
                        }; // rm released here

                        for sig in close_signals {
                            let mi = market_infos_refresh.values().find(|m| {
                                sig.symbol.contains(&m.symbol)
                                    || m.symbol.contains(&sig.symbol.replace("market_", ""))
                            });
                            let market_id = mi
                                .map(|m| m.market_id)
                                .or_else(|| market_id_of(&sig.symbol))
                                .unwrap_or(0);

                            info!(
                                "📌 {} — {} {:?} {:.6} @ {:.2} (entry={:.2})",
                                sig.reason,
                                sig.symbol,
                                sig.side_to_close,
                                sig.size,
                                sig.current_price,
                                sig.entry_price
                            );

                            // Cancel all orders first to free up margin
                            let _ = client_for_refresh.cancel_all_orders("all").await;
                            let _ = client_for_refresh.refresh_nonce().await;

                            match client_for_refresh
                                .place_order_with_market(
                                    market_id,
                                    sig.side_to_close,
                                    sig.current_price,
                                    sig.size,
                                    mi,
                                    true,
                                )
                                .await
                            {
                                Ok(resp) => {
                                    info!("✅ 止损止盈订单: id={} — {}", resp.order_id, sig.reason)
                                }
                                Err(e) => error!("❌ 止损止盈订单失败: {} — {}", e, sig.reason),
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Account refresh error: {}", e);
                }
            }
        }
    });

    info!("🎯 Trading system ready. Waiting for market data...");
    let mut ui_auto_feed_started = auto_universe;

    // Main event loop
    let mut trade_count: u64 = 0;
    let mut last_risk_update = std::time::Instant::now();
    while let Ok(msg) = ws_receiver.recv().await {
        // Update data store
        {
            let mut store = data_store_clone.write().await;
            match &msg {
                lighter::types::WsMessage::OrderBookUpdate(ob) => {
                    store.update_order_book(ob.clone());
                    if let Some(bbo) = hft::bbo_from_order_book(ob) {
                        latest_bbos.lock().unwrap().insert(bbo.market_id, bbo);
                    }
                }
                lighter::types::WsMessage::BboUpdate(bbo) => {
                    store.update_order_book(hft::order_book_from_bbo(bbo));
                    latest_bbos
                        .lock()
                        .unwrap()
                        .insert(bbo.market_id, bbo.clone());
                }
                lighter::types::WsMessage::TradeUpdate(trade) => {
                    store.add_trade(trade.clone());
                }
                _ => {}
            }
        }

        let dash_universe = {
            let ds = dash_state.read().await;
            ds.universe_mode.clone()
        };
        let auto_now = auto_universe || dash_universe == "auto" || dash_universe == "all";
        if auto_now && !ui_auto_feed_started {
            ui_auto_feed_started = true;
            let extra: Vec<u32> = hft::discover_perp_ids(&all_markets)
                .into_iter()
                .filter(|id| !subscribed.contains(id))
                .collect();
            if !extra.is_empty() {
                info!(
                    extra = extra.len(),
                    "Dashboard switched to auto universe; subscribing remaining perps"
                );
                if let Ok(shards) = hft::plan_subscription_shards(&extra, 100) {
                    for (shard_index, shard) in shards.into_iter().enumerate() {
                        let store = data_store_clone.clone();
                        let bbos = latest_bbos.clone();
                        let url = ws_url.clone();
                        tokio::spawn(async move {
                            let conn = lighter::websocket::LighterWebSocket::new(&url);
                            if conn.connect().await.is_err() {
                                return;
                            }
                            let mut receiver = conn.get_receiver();
                            for mid in &shard {
                                let _ = conn.subscribe_ticker(*mid).await;
                                tokio::time::sleep(std::time::Duration::from_millis(310)).await;
                            }
                            info!(shard = shard_index, "UI auto-universe ticker shard live");
                            while let Ok(message) = receiver.recv().await {
                                if let lighter::types::WsMessage::BboUpdate(bbo) = message {
                                    store
                                        .write()
                                        .await
                                        .update_order_book(hft::order_book_from_bbo(&bbo));
                                    bbos.lock().unwrap().insert(bbo.market_id, bbo);
                                }
                            }
                        });
                    }
                }
            }
        }
        if auto_now {
            let bbos: Vec<(u32, hft::BboUpdate)> = latest_bbos
                .lock()
                .unwrap()
                .iter()
                .map(|(id, bbo)| (*id, bbo.clone()))
                .collect();
            if !bbos.is_empty() {
                let rank_params = uni_params.clone();
                let resolved = hft::resolve_live_universe(
                    if auto_now { "auto" } else { &uni_mode },
                    &resolver_configured_ids,
                    &all_markets,
                    &bbos,
                    &rank_params,
                );
                let set_changed = {
                    use std::collections::HashSet;
                    let prev: HashSet<u32> = quote_ids.iter().copied().collect();
                    let next: HashSet<u32> = resolved.quoting_ids.iter().copied().collect();
                    if next.is_empty() && !prev.is_empty() {
                        false
                    } else if prev.is_empty() {
                        !next.is_empty()
                    } else {
                        let overlap = prev.intersection(&next).count();
                        // Tight crypto books all look similar; the 5th slot
                        // otherwise rotates every tick and cancel-alls the book.
                        let allowed_churn = 1usize;
                        overlap + allowed_churn < prev.len().max(next.len())
                    }
                };
                if set_changed {
                    info!(
                        "🎯 Auto universe re-ranked from live BBO: {:?} → {:?}",
                        quote_ids, resolved.quoting_ids
                    );
                    quote_ids = resolved.quoting_ids.clone();
                    lighter_client.set_active_markets(quote_ids.clone());
                    dash_state.write().await.active_markets = quote_ids.clone();
                }
            }
        }

        // Run strategy
        let mut snapshot = data_store_clone.read().await.get_snapshot();

        // 把盘口中间价写回 DashboardState。这是面板上唯一一个「空仓时也还在动」的数：
        // positions 里的 mark_price 只有持仓才有，而这个 bot 大部分时间是空仓的。
        //
        // 两点注意：
        // 1) 用 `ob.symbol` 而不是 map 的 key。二者当前相同（storage.rs 就是按
        //    order_book.symbol 插入的），但显式取字段可以免疫 key 格式漂移 ——
        //    键对不上时面板只会一直显示「暂无行情」，不会报错，很难查。
        // 2) 这里在 WS 驱动的主循环热路径上，而 DashboardState 是唯一的跨任务控制面。
        //    所以先只读比较、值真的变了才拿写锁；面板 3s 才消费一次，不会漏。
        {
            let mut prices = std::collections::HashMap::new();
            for ob in snapshot.order_books.values() {
                // 增量盘口更新的某一瞬间，买盘顶部可能还没补上，best_bid 会掉到很深的一档，
                // 中间价随之被拉歪（实测出现过 (63193+60040)/2 = 61616，偏离 2.5%）。
                // 价差超过 0.5% 就认为这一帧的盘口不完整，跳过不更新，保留上一个值。
                let (Some(bid), Some(ask), Some(mid)) =
                    (ob.best_bid(), ob.best_ask(), ob.mid_price())
                else {
                    continue;
                };
                if mid > 0.0 && bid > 0.0 && ask > bid && (ask - bid) / mid < 0.005 {
                    prices.insert(ob.symbol.clone(), mid);
                }
            }
            if !prices.is_empty() {
                let changed = {
                    let ds = dash_state.read().await;
                    ds.last_prices != prices
                };
                if changed {
                    // 整体赋值而非 extend：不再挂盘的市场会自动被剔除，不会无限累积
                    dash_state.write().await.last_prices = prices;
                }
            }
        }

        // Inject real exchange positions (signed: long = +, short = -) so the strategy
        // can cap one-sided accumulation against the actual position, not just its own
        // reset-prone internal state.
        {
            let ds = dash_state.read().await;
            snapshot.positions_authoritative = true;
            for p in &ds.positions {
                let (Some(symbol), Some(size), Some(side)) =
                    (p["symbol"].as_str(), p["size"].as_f64(), p["side"].as_str())
                else {
                    continue;
                };
                let signed = if side == "Sell" {
                    -size.abs()
                } else {
                    size.abs()
                };
                snapshot.positions.insert(symbol.to_string(), signed);
                if let Some(entry_price) = p["entry_price"].as_f64().filter(|price| *price > 0.0) {
                    snapshot
                        .position_entry_prices
                        .insert(symbol.to_string(), entry_price);
                }
            }
        }

        // Block all trading when emergency is active
        {
            let rm = risk_manager.lock().await;
            let is_emergency = rm.is_emergency_triggered();
            drop(rm);
            if is_emergency {
                if last_risk_update.elapsed().as_secs() >= 5 {
                    warn!("🚨 紧急模式 — 停止所有交易信号处理");
                    last_risk_update = std::time::Instant::now();
                }
                continue;
            }
        }

        // Check dashboard trading controls
        let (is_paused, active_markets, should_cancel_all) = {
            let ds = dash_state.read().await;
            (
                ds.trading_paused,
                ds.active_markets.clone(),
                ds.cancel_all_requested,
            )
        };

        // Handle cancel-all request from dashboard
        if should_cancel_all {
            info!("🗑️ Executing cancel-all from dashboard...");
            match lighter_client.cancel_all_orders("all").await {
                Ok(_) => info!("✅ All orders cancelled via dashboard"),
                Err(e) => warn!("⚠️ Cancel-all failed: {}", e),
            }
            let mut ds = dash_state.write().await;
            ds.cancel_all_requested = false;
        }

        // Check for risk config updates from dashboard
        {
            let mut ds = dash_state.write().await;
            if let Some(update) = ds.risk_update_requested.take() {
                let mut rm = risk_manager.lock().await;
                rm.update_params(
                    update.get("max_drawdown_pct").and_then(|v| v.as_f64()),
                    update.get("daily_loss_limit_pct").and_then(|v| v.as_f64()),
                    update.get("max_leverage").and_then(|v| v.as_f64()),
                    update
                        .get("position_stop_loss_pct")
                        .and_then(|v| v.as_f64()),
                    update
                        .get("position_take_profit_pct")
                        .and_then(|v| v.as_f64()),
                );
            }
        }

        // Block all trading when paused from dashboard
        if is_paused {
            continue;
        }

        // Check total position exposure — block same-direction signals if overleveraged
        let position_exposure: f64 = {
            let ds = dash_state.read().await;
            ds.positions
                .iter()
                .map(|p| {
                    let size = p["size"].as_f64().unwrap_or(0.0).abs();
                    let entry = p["entry_price"].as_f64().unwrap_or(0.0);
                    size * entry
                })
                .sum()
        };
        let equity = {
            let ds = dash_state.read().await;
            ds.equity
        };
        let current_leverage = if equity > 0.0 {
            position_exposure / equity
        } else {
            0.0
        };

        match strategy.read().await.evaluate(&snapshot).await {
            Ok(Some(signals)) => {
                let strat_name = strategy.read().await.name().to_string();
                if strat_name == "market_making" && !signals.is_empty() {
                    // Cancel-replace: never stack a new two-sided quote on top of the last.
                    match lighter_client.cancel_all_orders("all").await {
                        Ok(()) => {
                            open_orders_count.store(0, std::sync::atomic::Ordering::Relaxed);
                            let _ = lighter_client.refresh_nonce().await;
                        }
                        Err(e) => warn!("MM cancel-replace: {e}"),
                    }
                }
                for mut signal in signals {
                    // Check if market is active (dashboard trading controls)
                    if !active_markets.contains(&signal.market_id) {
                        continue;
                    }

                    // Check max open orders limit
                    let current_open = open_orders_count.load(std::sync::atomic::Ordering::Relaxed);
                    if current_open >= max_open_orders {
                        info!(
                            "⏸️ Max open orders ({}/{}) reached, skipping signal: {} {:?}",
                            current_open, max_open_orders, signal.symbol, signal.side
                        );
                        continue;
                    }

                    // Wait if grid is being reset (prevents nonce race)
                    if grid_resetting.load(std::sync::atomic::Ordering::Relaxed) {
                        debug!(
                            "⏳ Grid resetting, skipping signal: {} {:?}",
                            signal.symbol, signal.side
                        );
                        continue;
                    }

                    // Leverage limit: block new position-increasing signals if leverage > limit
                    let leverage_limit = {
                        let ds = dash_state.read().await;
                        ds.leverage_limit
                    };
                    if current_leverage > leverage_limit {
                        let existing_side: Option<String> = {
                            let ds = dash_state.read().await;
                            ds.positions
                                .iter()
                                .find(|p| {
                                    p["symbol"]
                                        .as_str()
                                        .map(|s| signal.symbol.contains(s))
                                        .unwrap_or(false)
                                })
                                .and_then(|p| p["side"].as_str().map(|s| s.to_string()))
                        };
                        let would_increase = matches!(
                            (existing_side.as_deref(), signal.side),
                            (Some("Buy"), lighter::types::Side::Buy)
                                | (Some("Sell"), lighter::types::Side::Sell)
                        );
                        if would_increase {
                            info!("⚠️ Leverage {:.1}x > {:.1}x limit, blocking same-direction signal: {} {:?}",
                                current_leverage, leverage_limit, signal.symbol, signal.side);
                            continue;
                        }
                    }

                    // Dedup: skip if an order already exists at a similar price (within 0.3%)
                    {
                        let ds = dash_state.read().await;
                        let has_dup = ds.open_orders_list.iter().any(|o| {
                            let same_symbol = o["symbol"].as_str() == Some(&signal.symbol);
                            let same_side = o["side"].as_str()
                                == Some(match signal.side {
                                    lighter::types::Side::Buy => "Buy",
                                    lighter::types::Side::Sell => "Sell",
                                });
                            let order_price = o["price"].as_f64().unwrap_or(0.0);
                            let price_diff = (order_price - signal.price).abs() / signal.price;
                            same_symbol && same_side && price_diff < 0.0008
                        });
                        if has_dup {
                            debug!(
                                "🔄 Skipping duplicate signal: {} {:?} @ {:.2}",
                                signal.symbol, signal.side, signal.price
                            );
                            continue;
                        }
                    }

                    // Risk check
                    {
                        let rm = risk_manager.lock().await;
                        if !rm.check_signal(&signal).await.unwrap_or(false) {
                            continue;
                        }
                    }

                    // Risk-reducing signals (exits/stop-loss): clamp quantity to the
                    // actual position held. Defense in depth on top of reduce_only
                    // — 即使交易所侧 ReduceOnly 生效，也要保证发出的数量语义正确，
                    // 防止策略状态滞后导致超大离场量（2026-08-08 事故的 qty 抬升隐患）。
                    if signal.risk_reducing {
                        let held_size = {
                            let ds = dash_state.read().await;
                            ds.positions
                                .iter()
                                .find(|p| {
                                    p["symbol"]
                                        .as_str()
                                        .map(|s| signal.symbol.contains(s))
                                        .unwrap_or(false)
                                })
                                .map(|p| p["size"].as_f64().unwrap_or(0.0).abs())
                                .unwrap_or(0.0)
                        };
                        if held_size <= 0.0 {
                            info!(
                                "⏭️ Risk-reducing signal for {} but no position held, skipping",
                                signal.symbol
                            );
                            continue;
                        }
                        if signal.quantity > held_size {
                            info!(
                                "🛡️ Clamping risk-reducing {} qty {:.6} -> held {:.6}",
                                signal.symbol, signal.quantity, held_size
                            );
                            signal.quantity = held_size;
                        }
                    }

                    let market_info = market_infos.get(&signal.market_id);
                    info!(
                        "📊 Signal: {} {:?} {} @ ${:.2} qty={:.6} — {}",
                        signal.symbol,
                        signal.side,
                        signal.market_id,
                        signal.price,
                        signal.quantity,
                        signal.reason
                    );

                    match lighter_client
                        .place_order_with_market(
                            signal.market_id,
                            signal.side,
                            signal.price,
                            signal.quantity,
                            market_info,
                            signal.risk_reducing,
                        )
                        .await
                    {
                        Ok(resp) => {
                            trade_count += 1;
                            // Optimistically increment open orders counter
                            open_orders_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            info!(
                                "✅ Order placed: id={}, status={}",
                                resp.order_id, resp.status
                            );

                            // Update dashboard
                            let mut ds = dash_state.write().await;
                            ds.total_trades = trade_count;
                            ds.open_orders =
                                open_orders_count.load(std::sync::atomic::Ordering::Relaxed);
                            // Determine action: Open (new position) or Add (increase existing)
                            let action = {
                                let has_position = ds.positions.iter().any(|p| {
                                    p.get("symbol").and_then(|s| s.as_str()) == Some(&signal.symbol)
                                });
                                if has_position {
                                    "Add"
                                } else {
                                    "Open"
                                }
                            };
                            // Shared path with close events: updates lifetime volume
                            // and trims to TRADE_HISTORY_LIMIT (was inconsistently 100 here).
                            // Disk flush happens on close events / periodic equity save.
                            ds.push_trade(serde_json::json!({
                                "timestamp": signal.timestamp.to_rfc3339(),
                                "symbol": signal.symbol,
                                "market_id": signal.market_id,
                                "side": format!("{:?}", signal.side),
                                "price": signal.price,
                                "quantity": signal.quantity,
                                "pnl": 0.0,
                                "action": action,
                                "reason": signal.reason,
                            }));
                        }
                        Err(e) => {
                            error!("❌ Order failed: {}", e);
                        }
                    }
                }
            }
            Ok(None) => {} // No signals
            Err(e) => {
                warn!("Strategy error: {}", e);
            }
        }

        // Update risk status in dashboard every 5 seconds (not every tick)
        if last_risk_update.elapsed() >= std::time::Duration::from_secs(5) {
            let rm = risk_manager.lock().await;
            let risk_status = rm.status();
            let is_emergency = rm.is_emergency_triggered();
            drop(rm);

            // If emergency triggered, stop processing new signals
            if is_emergency {
                let mut ds = dash_state.write().await;
                ds.risk_status = Some(serde_json::json!({
                    "drawdown_pct": risk_status.drawdown_pct,
                    "daily_loss_pct": risk_status.daily_loss_pct,
                    "max_drawdown_limit": risk_status.max_drawdown_limit,
                    "daily_loss_limit": risk_status.daily_loss_limit,
                    "position_stop_loss_pct": risk_status.position_stop_loss_pct,
                    "position_take_profit_pct": risk_status.position_take_profit_pct,
                    "is_healthy": risk_status.is_healthy,
                    "emergency_triggered": risk_status.emergency_triggered,
                }));
                drop(ds);
                warn!("🚨 紧急模式 — 停止所有交易信号处理");
                last_risk_update = std::time::Instant::now();
                continue;
            }

            let mut ds = dash_state.write().await;
            ds.risk_status = Some(serde_json::json!({
                "drawdown_pct": risk_status.drawdown_pct,
                "daily_loss_pct": risk_status.daily_loss_pct,
                "max_drawdown_limit": risk_status.max_drawdown_limit,
                "daily_loss_limit": risk_status.daily_loss_limit,
                "position_stop_loss_pct": risk_status.position_stop_loss_pct,
                "position_take_profit_pct": risk_status.position_take_profit_pct,
                "is_healthy": risk_status.is_healthy,
                "emergency_triggered": risk_status.emergency_triggered,
            }));
            drop(ds);
            last_risk_update = std::time::Instant::now();
        }

        // Check if dashboard user changed strategy params or switched strategy
        {
            let mut ds = dash_state.write().await;
            if ds.strategy_config_changed {
                ds.strategy_config_changed = false;
                let new_strategy_name = ds.strategy_name.clone();
                let params = ds.strategy_params.clone();
                drop(ds);

                // Build params string from HashMap
                let params_str: String = params
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(",");

                // Check if strategy type changed
                let current_name = strategy.read().await.name().to_string();
                if new_strategy_name != current_name && !new_strategy_name.is_empty() {
                    info!(
                        "🔄 Strategy switch: {} → {}",
                        current_name, new_strategy_name
                    );
                    match crate::strategy::create_strategy_with_params(
                        &new_strategy_name,
                        if params_str.is_empty() {
                            None
                        } else {
                            Some(&params_str)
                        },
                    ) {
                        Ok(new_strat) => {
                            *strategy.write().await = new_strat;
                            info!("✅ Strategy switched to: {}", new_strategy_name);
                        }
                        Err(e) => {
                            warn!(
                                "❌ Strategy switch failed: {} — keeping {}",
                                e, current_name
                            );
                            let mut ds = dash_state.write().await;
                            ds.strategy_name = current_name;
                        }
                    }
                } else if !params_str.is_empty() {
                    info!("🔧 Strategy params update: {:?}", params);
                    // Recreate with new params
                    match crate::strategy::create_strategy_with_params(
                        &current_name,
                        Some(&params_str),
                    ) {
                        Ok(new_strat) => {
                            *strategy.write().await = new_strat;
                            info!("✅ Strategy recreated with new params");
                        }
                        Err(e) => {
                            warn!(
                                "⚠ Failed to recreate strategy: {} — clearing state instead",
                                e
                            );
                            strategy.read().await.clear_filled_state();
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

struct BacktestCliOpts {
    output: Option<String>,
    params: Option<String>,
    config: Option<String>,
}

async fn run_backtest(
    strategy_name: &str,
    data_path: &str,
    start_date: &str,
    end_date: &str,
    initial_capital: f64,
    opts: BacktestCliOpts,
) -> Result<()> {
    info!("📊 Starting backtest: {}", strategy_name);
    info!("   Data: {}", data_path);
    info!("   Period: {} to {}", start_date, end_date);
    info!("   Capital: ${:.2}", initial_capital);
    if let Some(p) = &opts.params {
        info!("   Params: {}", p);
    }

    let historical_data = data::loader::load_csv_data_in_range(data_path, start_date, end_date)
        .context("Failed to load historical data")?;

    let mut backtest_engine =
        backtest::engine::BacktestEngine::new(initial_capital, historical_data);

    // 收益门槛 parity：--config 指定与实盘相同的 yaml 时，回测也拒绝净收益不足的入场
    if let Some(cfg) = &opts.config {
        let settings = Config::builder()
            .add_source(config::File::with_name(cfg))
            .build()
            .context("Failed to load config")?;
        let guard = risk::profitability::ProfitabilityGuard::from_config(&settings)?;
        let cost_bps = guard.total_cost_bps();
        backtest_engine = backtest_engine.with_profitability(guard);
        let commission = backtest::commission_rate_from_config(&settings);
        backtest_engine = backtest_engine.with_commission(commission);
        info!(
            "🧮 Profitability gate enabled (total cost {:.2} bps); commission floor {:.2} bps/side",
            cost_bps,
            commission * 10_000.0
        );
    }

    let bt_strategy = strategy::create_strategy_with_params(strategy_name, opts.params.as_deref())?;
    let results = backtest_engine.run(bt_strategy).await?;

    let output_path = opts.output.as_deref().unwrap_or("backtests/results");
    backtest::metrics::generate_report(&results, output_path).await?;

    info!("📈 Backtest complete!");
    info!("   Return: {:.2}%", results.total_return * 100.0);
    info!("   Sharpe: {:.3}", results.sharpe_ratio);
    info!("   Max DD: {:.2}%", results.max_drawdown * 100.0);
    info!("   Trades: {}", results.trades.len());
    info!("   Win Rate: {:.1}%", results.win_rate * 100.0);
    info!(
        "   Blocked by profitability gate: {}",
        results.blocked_by_profitability
    );

    Ok(())
}

/// Run parameter optimization sweep across grid strategy parameters
async fn run_optimize(
    strategy_name: &str,
    data_path: &str,
    start_date: &str,
    end_date: &str,
    initial_capital: f64,
    output_dir: Option<&str>,
    config_path: Option<&str>,
) -> Result<()> {
    info!("🔬 Starting parameter optimization for: {}", strategy_name);

    let historical_data = data::loader::load_csv_data_in_range(data_path, start_date, end_date)
        .context("Failed to load historical data")?;
    info!("   Loaded {} candles", historical_data.len());

    // 收益门槛 parity：--config 指定与实盘相同的 yaml 时，参数扫描也拒绝净收益不足的入场
    let (profitability, commission) = if let Some(cfg) = config_path {
        let settings = Config::builder()
            .add_source(config::File::with_name(cfg))
            .build()
            .context("Failed to load config")?;
        let guard = risk::profitability::ProfitabilityGuard::from_config(&settings)?;
        let cost_bps = guard.total_cost_bps();
        let commission = backtest::commission_rate_from_config(&settings);
        info!(
            "🧮 Profitability gate enabled (total cost {:.2} bps); commission floor {:.2} bps/side",
            cost_bps,
            commission * 10_000.0
        );
        (Some(guard), Some(commission))
    } else {
        (None, None)
    };

    // Define parameter grid based on strategy type
    let param_sets: Vec<String> = match strategy_name {
        "grid_trading" | "grid" => {
            let grid_counts = [6, 8, 10, 14, 20];
            let investments = [5.0, 8.0, 12.0, 16.0];
            let deviations = [0.003, 0.005, 0.008, 0.012, 0.02];
            let mut sets = Vec::new();
            for &gc in &grid_counts {
                for &inv in &investments {
                    for &dev in &deviations {
                        sets.push(format!(
                            "grid_count={},investment={},deviation={}",
                            gc, inv, dev
                        ));
                    }
                }
            }
            sets
        }
        "market_making" | "mm" => {
            let bid_spreads = [0.0005, 0.001, 0.002];
            let ask_spreads = [0.0005, 0.001, 0.002];
            let notionals = [25.0, 50.0, 100.0];
            let mut sets = Vec::new();
            for &bid in &bid_spreads {
                for &ask in &ask_spreads {
                    for &n in &notionals {
                        sets.push(format!(
                            "bid_spread={bid},ask_spread={ask},order_notional={n}"
                        ));
                    }
                }
            }
            sets
        }
        "trend_following" | "trend" => {
            let fast_periods = [5, 7, 10, 14];
            let slow_periods = [14, 21, 30, 50];
            let stop_losses = [0.02, 0.03, 0.05];
            let take_profits = [0.04, 0.06, 0.10];
            let trailing_stops = [0.0, 0.015, 0.025];
            let mut sets = Vec::new();
            for &f in &fast_periods {
                for &s in &slow_periods {
                    if f >= s {
                        continue;
                    }
                    for &sl in &stop_losses {
                        for &tp in &take_profits {
                            if tp <= sl {
                                continue;
                            }
                            for &tr in &trailing_stops {
                                sets.push(format!(
                                    "fast_ma={},slow_ma={},stop_loss={},take_profit={},trailing_stop={}",
                                    f, s, sl, tp, tr
                                ));
                            }
                        }
                    }
                }
            }
            sets
        }
        _ => anyhow::bail!("未知策略: {}", strategy_name),
    };

    info!("   Testing {} parameter combinations...", param_sets.len());

    struct OptResult {
        params: String,
        total_return: f64,
        sharpe: f64,
        max_dd: f64,
        trades: usize,
        win_rate: f64,
        profit_factor: f64,
    }

    let mut results_vec: Vec<OptResult> = Vec::new();

    for (i, params) in param_sets.iter().enumerate() {
        let bt_strategy = strategy::create_strategy_with_params(strategy_name, Some(params))?;
        let mut engine =
            backtest::engine::BacktestEngine::new(initial_capital, historical_data.clone());
        if let Some(guard) = &profitability {
            engine = engine.with_profitability(guard.clone());
        }
        if let Some(rate) = commission {
            engine = engine.with_commission(rate);
        }
        let result = engine.run(bt_strategy).await?;

        results_vec.push(OptResult {
            params: params.clone(),
            total_return: result.total_return,
            sharpe: result.sharpe_ratio,
            max_dd: result.max_drawdown,
            trades: result.total_trades,
            win_rate: result.win_rate,
            profit_factor: result.profit_factor,
        });

        if (i + 1) % 20 == 0 {
            info!("   Progress: {}/{}", i + 1, param_sets.len());
        }
    }

    // Sort by Sharpe ratio descending (highest = best risk-adjusted performance)
    results_vec.sort_by(|a, b| {
        let score_a = if a.trades > 0 {
            a.sharpe
        } else {
            f64::NEG_INFINITY
        };
        let score_b = if b.trades > 0 {
            b.sharpe
        } else {
            f64::NEG_INFINITY
        };
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Print top 10 results
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("🏆 TOP 10 PARAMETER COMBINATIONS");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    for (rank, r) in results_vec.iter().take(10).enumerate() {
        info!(
            "#{:2} Return: {:+6.2}% | Sharpe: {:6.3} | MaxDD: {:5.2}% | Trades: {:4} | WinRate: {:5.1}% | PF: {:5.2} | {}",
            rank + 1, r.total_return * 100.0, r.sharpe, r.max_dd * 100.0,
            r.trades, r.win_rate * 100.0, r.profit_factor, r.params
        );
    }
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    // Save full results to CSV
    let output_path = output_dir.unwrap_or("backtests/results");
    let opt_dir = format!("{}/optimization", output_path);
    std::fs::create_dir_all(&opt_dir)?;
    let csv_path = format!("{}/sweep_results.csv", opt_dir);
    let mut csv = String::from(
        "rank,params,return_pct,sharpe,max_dd_pct,trades,win_rate_pct,profit_factor\n",
    );
    for (i, r) in results_vec.iter().enumerate() {
        csv.push_str(&format!(
            "{},\"{}\",{:.4},{:.4},{:.4},{},{:.2},{:.4}\n",
            i + 1,
            r.params,
            r.total_return * 100.0,
            r.sharpe,
            r.max_dd * 100.0,
            r.trades,
            r.win_rate * 100.0,
            r.profit_factor
        ));
    }
    std::fs::write(&csv_path, csv)?;
    info!("📄 Full results saved to: {}", csv_path);

    // Run the best params and generate detailed report
    if let Some(best) = results_vec.first() {
        info!(
            "\n🥇 Running detailed backtest with best params: {}",
            best.params
        );
        let bt_strategy = strategy::create_strategy_with_params(strategy_name, Some(&best.params))?;
        let mut engine = backtest::engine::BacktestEngine::new(initial_capital, historical_data);
        if let Some(guard) = &profitability {
            engine = engine.with_profitability(guard.clone());
        }
        if let Some(rate) = commission {
            engine = engine.with_commission(rate);
        }
        let result = engine.run(bt_strategy).await?;
        let best_dir = format!("{}/best", opt_dir);
        backtest::metrics::generate_report(&result, &best_dir).await?;
        info!("📊 Best-params detailed report saved to: {}", best_dir);
    }

    Ok(())
}

async fn run_dashboard(host: &str, port: u16) -> Result<()> {
    info!("🌐 Starting dashboard at {}:{}", host, port);
    dashboard::server::start(host, port)
        .await
        .context("Dashboard failed")
}

async fn download_data(
    symbol: &str,
    interval: &str,
    start_date: &str,
    end_date: &str,
    base_url: &str,
    tag: Option<&str>,
) -> Result<()> {
    info!(
        "📥 Download data: {} {} {} {} ({})",
        symbol, interval, start_date, end_date, base_url
    );

    let start = data::loader::parse_range_start(start_date).context("Invalid start date")?;
    let end = match data::loader::parse_range_end(end_date).context("Invalid end date")? {
        data::loader::RangeEnd::Inclusive(dt) => dt,
        data::loader::RangeEnd::Exclusive(dt) => dt - chrono::Duration::seconds(1),
    };

    let secs_per_candle = match interval {
        "1m" => 60,
        "5m" => 300,
        "15m" => 900,
        "1h" => 3600,
        "4h" => 14400,
        "1d" => 86400,
        other => anyhow::bail!("Unsupported interval: {}", other),
    };

    let client = lighter::client::LighterClient::new("", "", base_url, "");

    // 从交易所动态解析 symbol -> market_id（优先 perp 市场）
    let all_markets = client
        .get_all_markets()
        .await
        .context("Failed to fetch market list")?;
    lighter::symbols::register_all(all_markets.iter().map(|m| (m.market_id, m.symbol.clone())));
    let upper = symbol.to_ascii_uppercase();
    let market_id = all_markets
        .iter()
        .filter(|m| m.symbol.to_ascii_uppercase() == upper)
        .min_by_key(|m| if m.market_type == "perp" { 0 } else { 1 })
        .map(|m| m.market_id)
        .ok_or_else(|| {
            let mut available: Vec<&str> = all_markets.iter().map(|m| m.symbol.as_str()).collect();
            available.sort();
            anyhow::anyhow!(
                "Symbol {} not found. Available: {}",
                symbol,
                available.join(", ")
            )
        })?;
    info!("   Resolved {} -> market_id {}", symbol, market_id);

    // API 单次最多返回 500 根，按窗口分页拉取
    const MAX_CANDLES_PER_REQ: i64 = 500;
    let chunk_secs = secs_per_candle as i64 * MAX_CANDLES_PER_REQ;
    let mut candles: Vec<lighter::types::Candlestick> = Vec::new();
    let mut chunk_start = start.timestamp();
    let end_ts = end.timestamp();
    while chunk_start <= end_ts {
        let chunk_end = (chunk_start + chunk_secs - secs_per_candle as i64).min(end_ts);
        let batch = client
            .get_candlesticks_in_range(
                market_id,
                interval,
                chunk_start,
                chunk_end,
                MAX_CANDLES_PER_REQ as u32,
            )
            .await
            .context("Failed to download candlestick data")?;
        info!(
            "   Chunk {} -> {}: {} candles",
            chrono::DateTime::from_timestamp(chunk_start, 0).unwrap(),
            chrono::DateTime::from_timestamp(chunk_end, 0).unwrap(),
            batch.len()
        );
        candles.extend(batch);
        chunk_start = chunk_end + secs_per_candle as i64;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    candles.retain(|c| c.timestamp >= start && c.timestamp <= end);
    candles.sort_by_key(|c| c.timestamp);
    candles.dedup_by_key(|c| c.timestamp);
    if candles.is_empty() {
        anyhow::bail!(
            "No candles returned for {} {} {} {}",
            symbol,
            interval,
            start_date,
            end_date
        );
    }

    let network_tag = tag.map(|t| t.to_string()).unwrap_or_else(|| {
        if base_url.contains("rh.lighter") {
            "rh".to_string()
        } else {
            "mainnet".to_string()
        }
    });
    let output_path = format!(
        "backtests/data/{}-{}-{}-{}-{}.csv",
        symbol.to_ascii_uppercase(),
        network_tag,
        interval,
        start.format("%Y%m%d"),
        end.with_timezone(&Utc).format("%Y%m%d")
    );
    data::loader::write_csv_data(&output_path, &candles)
        .context("Failed to write downloaded data")?;

    info!(
        "✅ Data download complete: {} ({} candles)",
        output_path,
        candles.len()
    );
    Ok(())
}

async fn generate_test_data(symbol: &str, days: u32) -> Result<()> {
    info!("🎲 Generating test data: {} {}d", symbol, days);
    data::loader::generate_synthetic_data(symbol, days).context("Failed to generate test data")?;
    info!("✅ Test data generated");
    Ok(())
}
