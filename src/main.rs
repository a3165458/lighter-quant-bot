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
use chrono::Utc;
use config::Config;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error, debug};

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
        #[arg(short, long, default_value = "config/settings.yaml")]
        config: String,
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
    },

    /// Generate test data
    GenerateData {
        #[arg(short, long, default_value = "BTCUSDT")]
        symbol: String,
        #[arg(short, long, default_value = "30")]
        days: u32,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    utils::logger::init_logger();
    dotenv::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Live { config } => run_live_trading(&config).await,
        Commands::Backtest { strategy, data, start, end, capital, output, params } => {
            run_backtest(&strategy, &data, &start, &end, capital, output.as_deref(), params.as_deref()).await
        }
        Commands::Optimize { strategy, data, start, end, capital, output } => {
            run_optimize(&strategy, &data, &start, &end, capital, output.as_deref()).await
        }
        Commands::Dashboard { host, port } => run_dashboard(&host, port).await,
        Commands::Download { symbol, interval, start, end } => {
            download_data(&symbol, &interval, &start, &end).await
        }
        Commands::GenerateData { symbol, days } => generate_test_data(&symbol, days).await,
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

    // Load credentials from env
    let secret_key = std::env::var("LIGHTER_SECRET_KEY")
        .context("LIGHTER_SECRET_KEY not set in .env")?;
    let account_index: i64 = std::env::var("LIGHTER_ACCOUNT_INDEX")
        .context("LIGHTER_ACCOUNT_INDEX not set in .env")?
        .parse()
        .context("Invalid LIGHTER_ACCOUNT_INDEX")?;
    let api_key_index: i32 = std::env::var("LIGHTER_API_KEY_INDEX")
        .context("LIGHTER_API_KEY_INDEX not set in .env")?
        .parse()
        .context("Invalid LIGHTER_API_KEY_INDEX")?;

    let rest_url = settings.get_string("lighter.rest_url")
        .unwrap_or_else(|_| "https://mainnet.zklighter.elliot.ai".to_string());
    let ws_url = settings.get_string("lighter.ws_url")
        .unwrap_or_else(|_| "wss://mainnet.zklighter.elliot.ai/stream".to_string());
    let chain_id = settings.get_int("lighter.chain_id").unwrap_or(304) as i32;

    let max_open_orders = settings.get_int("trading.max_open_orders").unwrap_or(8) as u32;
    info!("⚙️ Max open orders: {}", max_open_orders);

    // Initialize FFI signer (uses API secret key, not L1 private key)
    info!("🔑 Initializing signer...");
    lighter::ffi::init(&rest_url, &secret_key, chain_id, api_key_index, account_index)
        .context("Failed to initialize FFI signer")?;
    info!("✅ Signer initialized (account={}, api_key_index={})", account_index, api_key_index);

    // Create REST client (shared between main loop and refresh task)
    let lighter_client = Arc::new(lighter::client::LighterClient::new_with_account(
        &rest_url, account_index, api_key_index,
    ));

    // Fetch initial nonce
    let nonce = lighter_client.refresh_nonce().await
        .context("Failed to fetch nonce")?;
    info!("📋 Initial nonce: {}", nonce);

    // Fetch account info
    info!("📡 Fetching account info...");
    let account = lighter_client.get_account_info().await
        .context("Failed to fetch account info")?;
    let equity = account.total_equity;
    let free_balance = account.balances.first().map(|b| b.free).unwrap_or(0.0);
    info!("✅ Account connected — Equity: ${:.2}, Free: ${:.2}, Positions: {}",
        equity, free_balance, account.positions.len());

    // Cancel all existing orders for a clean start
    info!("🧹 Cancelling all existing orders...");
    match lighter_client.cancel_all_orders("all").await {
        Ok(()) => info!("✅ All existing orders cancelled"),
        Err(e) => warn!("⚠️ Cancel all orders: {} (may have no open orders)", e),
    }
    // Refresh nonce after cancel-all (it consumed one)
    let _ = lighter_client.refresh_nonce().await;

    // Get market configuration
    let markets: Vec<i64> = settings.get("trading.markets")
        .unwrap_or_else(|_| vec![0, 1]);
    let market_ids: Vec<u32> = markets.iter().map(|m| *m as u32).collect();

    // Fetch market info
    let mut market_infos = std::collections::HashMap::new();
    for &mid in &market_ids {
        match lighter_client.get_market_info(mid).await {
            Ok(mi) => {
                info!("📊 Market {}: {} (price_dec={}, size_dec={}, last=${:.2})",
                    mid, mi.symbol, mi.price_decimals, mi.size_decimals, mi.last_trade_price);
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
    let dash_state = Arc::new(RwLock::new(dashboard::server::DashboardState {
        equity,
        available_balance: free_balance,
        unrealized_pnl: account.positions.iter().map(|p| p.unrealized_pnl).sum(),
        strategy_name: String::new(),
        total_trades: 0,
        open_orders: 0,
        open_orders_list: Vec::new(),
        positions: account.positions.iter().map(|p| {
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
            })
        }).collect(),
        trade_history: Vec::new(),
        order_books: std::collections::HashMap::new(),
        risk_status: None,
        daily_realized_pnl: 0.0,
        total_realized_pnl: 0.0,
        initial_equity: equity,
        peak_equity: equity,
        equity_history: vec![(Utc::now().timestamp(), equity)],
        pnl_history: vec![(Utc::now().timestamp(), 0.0)],
        strategy_params: {
            let mut m = std::collections::HashMap::new();
            m.insert("grid_count".to_string(),
                settings.get_int("trading.strategies.grid_trading.grid_count").unwrap_or(10).to_string());
            m.insert("investment_per_grid".to_string(),
                settings.get_float("trading.strategies.grid_trading.investment_per_grid").unwrap_or(8.0).to_string());
            m.insert("price_deviation".to_string(),
                settings.get_float("trading.strategies.grid_trading.price_deviation").unwrap_or(0.012).to_string());
            m
        },
        strategy_config_changed: false,
        daily_pnl_map: std::collections::HashMap::new(),
        active_markets: market_ids.clone(),
        trading_paused: false,
        cancel_all_requested: false,
        available_markets: vec![
            (0, "ETH".to_string()),
            (1, "BTC".to_string()),
        ],
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
    }));

    // Restore persistent PnL data from disk
    if let Some(persisted) = dashboard::server::PersistentPnlData::load() {
        let mut ds = dash_state.write().await;
        ds.restore_pnl(&persisted);
    }

    // Start dashboard server
    let dash_port = settings.get_int("dashboard.port").unwrap_or(2028) as u16;
    let dash_state_clone = dash_state.clone();
    tokio::spawn(async move {
        if let Err(e) = dashboard::server::start_with_state("0.0.0.0", dash_port, dash_state_clone).await {
            error!("Dashboard error: {}", e);
        }
    });
    info!("🌐 Dashboard started on port {}", dash_port);

    // Initialize risk manager (shared for periodic equity updates)
    let risk_manager = Arc::new(tokio::sync::Mutex::new(
        risk::risk_manager::RiskManager::new(&settings)
            .context("Failed to initialize risk manager")?
    ));
    {
        let mut rm = risk_manager.lock().await;
        rm.update_equity(equity);
    }
    // Sync initial risk config from RiskManager to DashboardState
    {
        let rm = risk_manager.lock().await;
        let mut ds = dash_state.write().await;
        ds.risk_config = rm.get_config();
        ds.risk_config["leverage_limit"] = serde_json::json!(ds.leverage_limit);
    }

    // Initialize strategy (wrapped in Arc<RwLock> for runtime switching)
    let strategy: Arc<tokio::sync::RwLock<Box<dyn strategy::Strategy>>> = Arc::new(
        tokio::sync::RwLock::new(strategy::create_strategy(&settings)
            .context("Failed to initialize strategy")?)
    );
    let strategy_name = strategy.read().await.name().to_string();
    info!("📈 Strategy: {}", strategy_name);

    // Update dashboard with strategy name
    {
        let mut ds = dash_state.write().await;
        ds.strategy_name = strategy_name.clone();
    }

    // Create data store
    let data_store = Arc::new(RwLock::new(data::storage::MarketDataStore::new()));

    // Fetch initial candle data for strategies that need history
    for &mid in &market_ids {
        let symbol = market_infos.get(&mid).map(|m| m.symbol.as_str()).unwrap_or("UNKNOWN");
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

    // Connect WebSocket
    info!("🔌 Connecting WebSocket...");
    let ws_client = lighter::websocket::LighterWebSocket::new(&ws_url);
    ws_client.connect().await?;

    // Subscribe to market data (trading markets + display-only markets)
    let display_markets: Vec<u32> = vec![0, 1]; // ETH=0, BTC=1 for dashboard orderbook
    let mut subscribed = std::collections::HashSet::new();
    for &mid in &market_ids {
        let symbol = market_infos.get(&mid).map(|m| m.symbol.as_str()).unwrap_or("?");
        ws_client.subscribe_market_data(&mid.to_string()).await?;
        subscribed.insert(mid);
        info!("📡 Subscribed to {} (market {})", symbol, mid);
    }
    for &mid in &display_markets {
        if !subscribed.contains(&mid) {
            ws_client.subscribe_market_data(&mid.to_string()).await?;
            info!("📡 Subscribed to market {} (display only)", mid);
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
    let configured_market_ids = market_ids.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        let mut at_max_since: Option<std::time::Instant> = None;
        // Equity-based realized PnL tracking
        let mut prev_equity: f64 = 0.0;
        let mut prev_unrealized: f64 = 0.0;
        // Position snapshot for logging close events (side, size, entry_price)
        let mut prev_positions: std::collections::HashMap<String, (lighter::types::Side, f64, f64)> = std::collections::HashMap::new();
        // Track daily PnL reset
        let mut last_daily_reset_day: u32 = (Utc::now().timestamp() / 86400) as u32;
        let mut first_cycle = true;
        loop {
            interval.tick().await;

            // Always sync real open orders count first (fast, lightweight)
            match client_for_refresh.get_open_orders("all").await {
                Ok(orders) => {
                    let count = orders.len() as u32;
                    let prev = open_orders_refresh.swap(count, std::sync::atomic::Ordering::Relaxed);
                    if prev != count {
                        info!("📋 Open orders synced: {} → {} (real)", prev, count);
                    }
                    let mut ds = dash_state_refresh.write().await;
                    ds.open_orders = count;
                    // Also store the actual orders for dashboard display
                    ds.open_orders_list = orders.iter().map(|o| serde_json::json!({
                        "id": o.id,
                        "symbol": o.symbol,
                        "side": format!("{:?}", o.side),
                        "price": o.price,
                        "quantity": o.quantity,
                        "filled_quantity": o.filled_quantity,
                        "status": format!("{:?}", o.status),
                    })).collect();
                    drop(ds);

                    // ===== Stale order management =====
                    // SKIP during emergency — don't cancel close orders
                    let is_emergency = {
                        let rm = risk_manager_refresh.lock().await;
                        rm.is_emergency_triggered()
                    };
                    if count > 0 && !is_emergency {
                        // Get fresh market prices
                        let mut mid_prices: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
                        for &mid in &configured_market_ids {
                            if let Ok(mi) = client_for_refresh.get_market_info(mid).await {
                                if mi.last_trade_price > 0.0 {
                                    mid_prices.insert(mid, mi.last_trade_price);
                                }
                            }
                        }

                        // Strategy 1: Cancel individual orders that are too far from mid price
                        let mut cancelled = 0u32;
                        for order in &orders {
                            let market_id = if order.symbol == "ETH" { 0u32 } else { 1u32 };
                            if let Some(&mid) = mid_prices.get(&market_id) {
                                let diff = (order.price - mid).abs() / mid;
                                if diff > stale_price_pct {
                                    if let Ok(idx) = order.id.parse::<i64>() {
                                        info!("🗑️ Cancelling stale order: {} {:?} @ {:.2} (mid={:.2}, diff={:.1}%)",
                                            order.symbol, order.side, order.price, mid, diff * 100.0);
                                        match client_for_refresh.cancel_order_by_index(market_id, idx).await {
                                            Ok(()) => cancelled += 1,
                                            Err(e) => warn!("Failed to cancel stale order {}: {}", order.id, e),
                                        }
                                    }
                                }
                            }
                        }

                        if cancelled > 0 {
                            info!("🗑️ Cancelled {} stale orders, resetting grid state", cancelled);
                            strategy_refresh.read().await.clear_filled_state();
                            let _ = client_for_refresh.refresh_nonce().await;
                            let new_count = count.saturating_sub(cancelled);
                            open_orders_refresh.store(new_count, std::sync::atomic::Ordering::Relaxed);
                            at_max_since = None; // Reset timer
                        }

                        // Strategy 2: Time-based cancel-all if orders sit unfilled too long
                        // This prevents the bot from being stuck when orders are within
                        // the grid range but none are filling
                        if count >= 3 { // Any meaningful number of open orders
                            if at_max_since.is_none() {
                                at_max_since = Some(std::time::Instant::now());
                            }
                            if let Some(since) = at_max_since {
                                let elapsed = since.elapsed().as_secs();
                                if elapsed >= max_order_age_secs {
                                    info!("🔄 Auto-reset: cancelling all {} orders (stale for {}s)", count, elapsed);
                                    grid_resetting_refresh.store(true, std::sync::atomic::Ordering::Relaxed);
                                    match client_for_refresh.cancel_all_orders("all").await {
                                        Ok(()) => {
                                            info!("✅ Auto-reset: all orders cancelled, re-gridding");
                                            strategy_refresh.read().await.clear_filled_state();
                                            let _ = client_for_refresh.refresh_nonce().await;
                                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                            open_orders_refresh.store(0, std::sync::atomic::Ordering::Relaxed);
                                            at_max_since = None;
                                        }
                                        Err(e) => warn!("❌ Auto-reset cancel failed: {}", e),
                                    }
                                    grid_resetting_refresh.store(false, std::sync::atomic::Ordering::Relaxed);
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
                    let curr_unrealized: f64 = acct.positions.iter().map(|p| p.unrealized_pnl).sum();

                    // ===== Auto-close positions on non-configured markets =====
                    for pos in &acct.positions {
                        if pos.size.abs() < 1e-10 { continue; }
                        let pos_market_id = if pos.symbol == "ETH" { 0u32 } else { 1u32 };
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
                            let fresh_price = match client_for_refresh.get_market_info(pos_market_id).await {
                                Ok(fmi) if fmi.last_trade_price > 0.0 => fmi.last_trade_price,
                                _ => pos.entry_price,
                            };
                            let slippage = 0.005;
                            let close_price = match close_side {
                                lighter::types::Side::Buy => fresh_price * (1.0 + slippage),
                                lighter::types::Side::Sell => fresh_price * (1.0 - slippage),
                            };
                            warn!("🔄 Auto-closing {} {:?} {:.6} @ {:.2} (non-configured market)",
                                pos.symbol, close_side, pos.size, close_price);
                            match client_for_refresh.place_order_with_market(
                                pos_market_id, close_side, close_price, pos.size.abs(), mi,
                            ).await {
                                Ok(resp) => info!("✅ Auto-close order placed: {} id={}", pos.symbol, resp.order_id),
                                Err(e) => error!("❌ Auto-close failed for {}: {}", pos.symbol, e),
                            }
                        }
                    }

                    // ===== Realized PnL detection =====
                    // Step 1: Detect actual position changes (size or side changed)
                    // Step 2: Only then compute PnL via equity method: realized = Δequity - Δunrealized
                    let mut realized_pnl_this_cycle = 0.0_f64;
                    let mut close_events: Vec<serde_json::Value> = Vec::new();

                    if !first_cycle && prev_equity > 0.0 {
                        // Build current position map with rounded sizes
                        let mut curr_pos_map: std::collections::HashMap<String, (lighter::types::Side, f64, f64)> = std::collections::HashMap::new();
                        for p in &acct.positions {
                            if p.size.abs() > 1e-10 {
                                // Round to market precision to avoid float jitter
                                let decimals = if p.symbol == "ETH" { 4 } else { 5 };
                                let factor = 10_f64.powi(decimals);
                                let rounded_size = (p.size * factor).round() / factor;
                                curr_pos_map.insert(p.symbol.clone(), (p.side, rounded_size, p.entry_price));
                            }
                        }

                        // Check for meaningful position changes (size decreased or position closed)
                        let mut position_reductions: Vec<(String, lighter::types::Side, f64, f64, &str)> = Vec::new();
                        for (symbol, (prev_side, prev_size, prev_entry)) in &prev_positions {
                            // Min change threshold per market
                            let min_change = if symbol == "ETH" { 0.0049 } else { 0.00019 };
                            match curr_pos_map.get(symbol) {
                                Some((curr_side, curr_size, _)) => {
                                    if *curr_side != *prev_side {
                                        // Side flipped — full close of old position
                                        position_reductions.push((symbol.clone(), *prev_side, *prev_size, *prev_entry, "Full Close"));
                                    } else if *prev_size - *curr_size >= min_change {
                                        // Position reduced
                                        let closed = prev_size - curr_size;
                                        let close_type = if *curr_size < min_change { "Full Close" } else { "Partial Close" };
                                        position_reductions.push((symbol.clone(), *prev_side, closed, *prev_entry, close_type));
                                    }
                                }
                                None => {
                                    // Position gone entirely
                                    if *prev_size >= 1e-10 {
                                        position_reductions.push((symbol.clone(), *prev_side, *prev_size, *prev_entry, "Full Close"));
                                    }
                                }
                            }
                        }

                        // Only compute PnL if an actual position change occurred
                        if !position_reductions.is_empty() {
                            let equity_change = curr_equity - prev_equity;
                            let unrealized_change = curr_unrealized - prev_unrealized;
                            realized_pnl_this_cycle = equity_change - unrealized_change;

                            // Distribute PnL across changed positions proportionally
                            let total_notional: f64 = position_reductions.iter()
                                .map(|(_, _, size, entry, _)| size * entry)
                                .sum();

                            for (symbol, prev_side, closed_size, prev_entry, close_type) in &position_reductions {
                                let pnl_share = if total_notional > 0.0 && position_reductions.len() > 1 {
                                    realized_pnl_this_cycle * (closed_size * prev_entry) / total_notional
                                } else {
                                    realized_pnl_this_cycle
                                };
                                let market_id = if symbol == "ETH" { 0u32 } else { 1u32 };

                                info!("💰 {} {}: {:?} {:.6} @ entry={:.2} | PnL: {}{:.4}",
                                    close_type, symbol, prev_side, closed_size, prev_entry,
                                    if pnl_share >= 0.0 { "+" } else { "" }, pnl_share);

                                close_events.push(serde_json::json!({
                                    "timestamp": Utc::now().to_rfc3339(),
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
                                }));
                            }
                        }
                    }
                    first_cycle = false;
                    prev_equity = curr_equity;
                    prev_unrealized = curr_unrealized;

                    // Update position snapshot (rounded) for next cycle
                    prev_positions.clear();
                    for p in &acct.positions {
                        if p.size.abs() > 1e-10 {
                            let decimals = if p.symbol == "ETH" { 4 } else { 5 };
                            let factor = 10_f64.powi(decimals);
                            let rounded_size = (p.size * factor).round() / factor;
                            prev_positions.insert(p.symbol.clone(), (p.side, rounded_size, p.entry_price));
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
                                info!("📅 New day — resetting daily realized PnL ({:.4} → 0.0)", ds.daily_realized_pnl);
                                ds.daily_realized_pnl = 0.0;
                                last_daily_reset_day = today;
                            }
                            ds.daily_realized_pnl += realized_pnl_this_cycle;
                            ds.total_realized_pnl += realized_pnl_this_cycle;
                            info!("📊 Realized PnL update: cycle={:+.4}, daily={:+.4}, total={:+.4}",
                                realized_pnl_this_cycle, ds.daily_realized_pnl, ds.total_realized_pnl);
                            // Persist to disk on every PnL change
                            ds.save_pnl();
                        }

                        // Record close events in trade history
                        for evt in close_events {
                            ds.trade_history.push(evt);
                        }
                        let len = ds.trade_history.len();
                        if len > 200 {
                            ds.trade_history.drain(..len - 200);
                        }

                        // Track initial equity on first update
                        if ds.initial_equity == 0.0 {
                            ds.initial_equity = curr_equity;
                        }
                        // Track peak equity
                        if curr_equity > ds.peak_equity {
                            ds.peak_equity = curr_equity;
                        }
                        // Record equity history (max 1440 points = 24h at 1/min)
                        let now_ts = Utc::now().timestamp();
                        let should_record = ds.equity_history.last()
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
                            let mut fresh_prices: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
                            // Fetch prices for all known markets (positions may exist on any)
                            for &mid in &[0u32, 1u32] {
                                if let Ok(fmi) = client_for_refresh.get_market_info(mid).await {
                                    if fmi.last_trade_price > 0.0 {
                                        fresh_prices.insert(mid, fmi.last_trade_price);
                                    }
                                }
                            }

                            // Close all positions at CURRENT market price with slippage
                            for pos in &acct.positions {
                                if pos.size.abs() < 1e-10 { continue; }
                                let close_side = match pos.side {
                                    lighter::types::Side::Buy => lighter::types::Side::Sell,
                                    lighter::types::Side::Sell => lighter::types::Side::Buy,
                                };
                                let mi = market_infos_refresh.values().find(|m| {
                                    pos.symbol.contains(&m.symbol) || m.symbol.contains(&pos.symbol.replace("market_", ""))
                                });
                                let market_id = mi.map(|m| m.market_id).unwrap_or(0);

                                // Use CURRENT market price + slippage (not entry price!)
                                let current_mid = fresh_prices.get(&market_id).copied().unwrap_or(pos.entry_price);
                                let slippage = 0.005; // 0.5% slippage for aggressive fill
                                let close_price = match close_side {
                                    lighter::types::Side::Buy => current_mid * (1.0 + slippage),
                                    lighter::types::Side::Sell => current_mid * (1.0 - slippage),
                                };

                                warn!("🚨 紧急平仓: {} {:?} {:.6} @ {:.2} (mid={:.2}, entry={:.2})",
                                    pos.symbol, close_side, pos.size, close_price, current_mid, pos.entry_price);
                                match client_for_refresh.place_order_with_market(
                                    market_id, close_side, close_price, pos.size.abs(), mi,
                                ).await {
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
                                current_prices.insert(format!("market_{}", mi.market_id), mi.last_trade_price);
                            }
                        }

                        for (&mid, mi) in &market_infos_refresh {
                            if let Ok(fresh_mi) = client_for_refresh.get_market_info(mid).await {
                                current_prices.insert(fresh_mi.symbol.clone(), fresh_mi.last_trade_price);
                                current_prices.insert(format!("market_{}", mid), fresh_mi.last_trade_price);
                            } else {
                                current_prices.insert(mi.symbol.clone(), mi.last_trade_price);
                                current_prices.insert(format!("market_{}", mid), mi.last_trade_price);
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
                                sig.symbol.contains(&m.symbol) || m.symbol.contains(&sig.symbol.replace("market_", ""))
                            });
                            let market_id = mi.map(|m| m.market_id).unwrap_or(0);

                            info!("📌 {} — {} {:?} {:.6} @ {:.2} (entry={:.2})",
                                sig.reason, sig.symbol, sig.side_to_close, sig.size, sig.current_price, sig.entry_price);

                            // Cancel all orders first to free up margin
                            let _ = client_for_refresh.cancel_all_orders("all").await;
                            let _ = client_for_refresh.refresh_nonce().await;

                            match client_for_refresh.place_order_with_market(
                                market_id, sig.side_to_close, sig.current_price, sig.size, mi,
                            ).await {
                                Ok(resp) => info!("✅ 止损止盈订单: id={} — {}", resp.order_id, sig.reason),
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

    // Main event loop
    let mut trade_count: u64 = 0;
    let mut last_risk_update = std::time::Instant::now();
    while let Ok(msg) = ws_receiver.recv().await {
        // Update data store
        {
            let mut store = data_store_clone.write().await;
            match &msg {
                lighter::types::WsMessage::OrderBookUpdate(ob) => {
                    // Update dashboard orderbook
                    let mut ds = dash_state.write().await;
                    ds.order_books.insert(ob.market_id, serde_json::json!({
                        "market_id": ob.market_id,
                        "bids": ob.bids.iter().take(10).map(|l| serde_json::json!({
                            "price": l.price,
                            "size": l.quantity,
                        })).collect::<Vec<_>>(),
                        "asks": ob.asks.iter().take(10).map(|l| serde_json::json!({
                            "price": l.price,
                            "size": l.quantity,
                        })).collect::<Vec<_>>(),
                    }));
                    drop(ds);
                    store.update_order_book(ob.clone());
                }
                lighter::types::WsMessage::TradeUpdate(trade) => {
                    store.add_trade(trade.clone());
                }
                _ => {}
            }
        }

        // Run strategy
        let snapshot = data_store_clone.read().await.get_snapshot();

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
            (ds.trading_paused, ds.active_markets.clone(), ds.cancel_all_requested)
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
                    update.get("position_stop_loss_pct").and_then(|v| v.as_f64()),
                    update.get("position_take_profit_pct").and_then(|v| v.as_f64()),
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
            ds.positions.iter().map(|p| {
                let size = p["size"].as_f64().unwrap_or(0.0).abs();
                let entry = p["entry_price"].as_f64().unwrap_or(0.0);
                size * entry
            }).sum()
        };
        let equity = {
            let ds = dash_state.read().await;
            ds.equity
        };
        let current_leverage = if equity > 0.0 { position_exposure / equity } else { 0.0 };

        match strategy.read().await.evaluate(&snapshot).await {
            Ok(Some(signals)) => {
                for signal in signals {
                    // Check if market is active (dashboard trading controls)
                    if !active_markets.contains(&signal.market_id) {
                        continue;
                    }

                    // Check max open orders limit
                    let current_open = open_orders_count.load(std::sync::atomic::Ordering::Relaxed);
                    if current_open >= max_open_orders {
                        info!("⏸️ Max open orders ({}/{}) reached, skipping signal: {} {:?}",
                            current_open, max_open_orders, signal.symbol, signal.side);
                        continue;
                    }

                    // Wait if grid is being reset (prevents nonce race)
                    if grid_resetting.load(std::sync::atomic::Ordering::Relaxed) {
                        debug!("⏳ Grid resetting, skipping signal: {} {:?}", signal.symbol, signal.side);
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
                            ds.positions.iter()
                                .find(|p| p["symbol"].as_str().map(|s| signal.symbol.contains(s)).unwrap_or(false))
                                .and_then(|p| p["side"].as_str().map(|s| s.to_string()))
                        };
                        let would_increase = match (existing_side.as_deref(), &signal.side) {
                            (Some("Buy"), &lighter::types::Side::Buy) => true,
                            (Some("Sell"), &lighter::types::Side::Sell) => true,
                            _ => false,
                        };
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
                            let same_side = o["side"].as_str() == Some(match signal.side {
                                lighter::types::Side::Buy => "Buy",
                                lighter::types::Side::Sell => "Sell",
                            });
                            let order_price = o["price"].as_f64().unwrap_or(0.0);
                            let price_diff = (order_price - signal.price).abs() / signal.price;
                            same_symbol && same_side && price_diff < 0.0008
                        });
                        if has_dup {
                            debug!("🔄 Skipping duplicate signal: {} {:?} @ {:.2}", signal.symbol, signal.side, signal.price);
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

                    let market_info = market_infos.get(&signal.market_id);
                    info!("📊 Signal: {} {:?} {} @ ${:.2} qty={:.6} — {}",
                        signal.symbol, signal.side, signal.market_id,
                        signal.price, signal.quantity, signal.reason);

                    match lighter_client.place_order_with_market(
                        signal.market_id,
                        signal.side,
                        signal.price,
                        signal.quantity,
                        market_info,
                    ).await {
                        Ok(resp) => {
                            trade_count += 1;
                            // Optimistically increment open orders counter
                            open_orders_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            info!("✅ Order placed: id={}, status={}", resp.order_id, resp.status);

                            // Update dashboard
                            let mut ds = dash_state.write().await;
                            ds.total_trades = trade_count;
                            ds.open_orders = open_orders_count.load(std::sync::atomic::Ordering::Relaxed);
                            // Determine action: Open (new position) or Add (increase existing)
                            let action = {
                                let has_position = ds.positions.iter().any(|p| {
                                    p.get("symbol").and_then(|s| s.as_str()) == Some(&signal.symbol)
                                });
                                if has_position { "Add" } else { "Open" }
                            };
                            ds.trade_history.push(serde_json::json!({
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
                            // Keep only last 100 trades
                            let len = ds.trade_history.len();
                            if len > 100 {
                                ds.trade_history.drain(..len - 100);
                            }
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
                let params_str: String = params.iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(",");

                // Check if strategy type changed
                let current_name = strategy.read().await.name().to_string();
                if new_strategy_name != current_name && !new_strategy_name.is_empty() {
                    info!("🔄 Strategy switch: {} → {}", current_name, new_strategy_name);
                    match crate::strategy::create_strategy_with_params(
                        &new_strategy_name,
                        if params_str.is_empty() { None } else { Some(&params_str) }
                    ) {
                        Ok(new_strat) => {
                            *strategy.write().await = new_strat;
                            info!("✅ Strategy switched to: {}", new_strategy_name);
                        }
                        Err(e) => {
                            warn!("❌ Strategy switch failed: {} — keeping {}", e, current_name);
                            let mut ds = dash_state.write().await;
                            ds.strategy_name = current_name;
                        }
                    }
                } else if !params_str.is_empty() {
                    info!("🔧 Strategy params update: {:?}", params);
                    // Recreate with new params
                    match crate::strategy::create_strategy_with_params(
                        &current_name,
                        Some(&params_str)
                    ) {
                        Ok(new_strat) => {
                            *strategy.write().await = new_strat;
                            info!("✅ Strategy recreated with new params");
                        }
                        Err(e) => {
                            warn!("⚠ Failed to recreate strategy: {} — clearing state instead", e);
                            strategy.read().await.clear_filled_state();
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
    params: Option<&str>,
) -> Result<()> {
    info!("📊 Starting backtest: {}", strategy_name);
    info!("   Data: {}", data_path);
    info!("   Period: {} to {}", start_date, end_date);
    info!("   Capital: ${:.2}", initial_capital);
    if let Some(p) = params {
        info!("   Params: {}", p);
    }

    let historical_data = data::loader::load_csv_data_in_range(data_path, start_date, end_date)
        .context("Failed to load historical data")?;

    let mut backtest_engine = backtest::engine::BacktestEngine::new(
        initial_capital,
        historical_data,
    );

    let bt_strategy = strategy::create_strategy_with_params(strategy_name, params)?;
    let results = backtest_engine.run(bt_strategy).await?;

    let output_path = output_dir.unwrap_or("backtests/results");
    backtest::metrics::generate_report(&results, output_path).await?;

    info!("📈 Backtest complete!");
    info!("   Return: {:.2}%", results.total_return * 100.0);
    info!("   Sharpe: {:.3}", results.sharpe_ratio);
    info!("   Max DD: {:.2}%", results.max_drawdown * 100.0);
    info!("   Trades: {}", results.trades.len());
    info!("   Win Rate: {:.1}%", results.win_rate * 100.0);

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
) -> Result<()> {
    info!("🔬 Starting parameter optimization for: {}", strategy_name);

    let historical_data = data::loader::load_csv_data_in_range(data_path, start_date, end_date)
        .context("Failed to load historical data")?;
    info!("   Loaded {} candles", historical_data.len());

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
                        sets.push(format!("grid_count={},investment={},deviation={}", gc, inv, dev));
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
            let mut sets = Vec::new();
            for &f in &fast_periods {
                for &s in &slow_periods {
                    if f >= s { continue; }
                    for &sl in &stop_losses {
                        for &tp in &take_profits {
                            if tp <= sl { continue; }
                            sets.push(format!("fast_ma={},slow_ma={},stop_loss={},take_profit={}", f, s, sl, tp));
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
        let mut engine = backtest::engine::BacktestEngine::new(
            initial_capital,
            historical_data.clone(),
        );
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
        let score_a = if a.trades > 0 { a.sharpe } else { f64::NEG_INFINITY };
        let score_b = if b.trades > 0 { b.sharpe } else { f64::NEG_INFINITY };
        score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
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
    let mut csv = String::from("rank,params,return_pct,sharpe,max_dd_pct,trades,win_rate_pct,profit_factor\n");
    for (i, r) in results_vec.iter().enumerate() {
        csv.push_str(&format!(
            "{},\"{}\",{:.4},{:.4},{:.4},{},{:.2},{:.4}\n",
            i + 1, r.params, r.total_return * 100.0, r.sharpe, r.max_dd * 100.0,
            r.trades, r.win_rate * 100.0, r.profit_factor
        ));
    }
    std::fs::write(&csv_path, csv)?;
    info!("📄 Full results saved to: {}", csv_path);

    // Run the best params and generate detailed report
    if let Some(best) = results_vec.first() {
        info!("\n🥇 Running detailed backtest with best params: {}", best.params);
        let bt_strategy = strategy::create_strategy_with_params(strategy_name, Some(&best.params))?;
        let mut engine = backtest::engine::BacktestEngine::new(
            initial_capital,
            historical_data,
        );
        let result = engine.run(bt_strategy).await?;
        let best_dir = format!("{}/best", opt_dir);
        backtest::metrics::generate_report(&result, &best_dir).await?;
        info!("📊 Best-params detailed report saved to: {}", best_dir);
    }

    Ok(())
}

async fn run_dashboard(host: &str, port: u16) -> Result<()> {
    info!("🌐 Starting dashboard at {}:{}", host, port);
    dashboard::server::start(host, port).await
        .context("Dashboard failed")
}

async fn download_data(
    symbol: &str,
    interval: &str,
    start_date: &str,
    end_date: &str,
) -> Result<()> {
    info!("📥 Download data: {} {} {} {}", symbol, interval, start_date, end_date);

    let market_id = match symbol.to_ascii_uppercase().as_str() {
        "ETH" => 0,
        "BTC" => 1,
        other => anyhow::bail!("Unsupported symbol for Lighter download: {}", other),
    };

    let start = data::loader::parse_range_start(start_date)
        .context("Invalid start date")?;
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

    let span_secs = (end - start).num_seconds().max(secs_per_candle as i64);
    let count_back = ((span_secs / secs_per_candle as i64) + 8) as u32;

    let client = lighter::client::LighterClient::new(
        "",
        "",
        "https://mainnet.zklighter.elliot.ai",
        "wss://mainnet.zklighter.elliot.ai/stream",
    );
    let mut candles = client
        .get_candlesticks_in_range(
            market_id,
            interval,
            start.timestamp(),
            end.timestamp(),
            count_back,
        )
        .await
        .context("Failed to download candlestick data")?;

    candles.retain(|c| c.timestamp >= start && c.timestamp <= end);
    if candles.is_empty() {
        anyhow::bail!("No candles returned for {} {} {} {}", symbol, interval, start_date, end_date);
    }

    let output_path = format!(
        "backtests/data/{}-{}-{}-{}.csv",
        symbol.to_ascii_uppercase(),
        interval,
        start.format("%Y%m%d"),
        end.with_timezone(&Utc).format("%Y%m%d")
    );
    data::loader::write_csv_data(&output_path, &candles)
        .context("Failed to write downloaded data")?;

    info!("✅ Data download complete: {} ({} candles)", output_path, candles.len());
    Ok(())
}

async fn generate_test_data(symbol: &str, days: u32) -> Result<()> {
    info!("🎲 Generating test data: {} {}d", symbol, days);
    data::loader::generate_synthetic_data(symbol, days)
        .context("Failed to generate test data")?;
    info!("✅ Test data generated");
    Ok(())
}
