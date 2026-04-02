use reqwest::Client;
use serde_json::Value;
use std::sync::atomic::{AtomicI64, Ordering};
use tracing::{debug, info, error, warn};

use super::error::LighterError;
use super::ffi;
use super::types::*;

/// Lighter REST API client
pub struct LighterClient {
    client: Client,
    base_url: String,
    account_index: i64,
    api_key_index: i32,
    nonce: AtomicI64,
}

impl LighterClient {
    /// Primary constructor for the Lighter REST API client.
    /// `base_url` is the REST API base URL, `account_index` is the account index,
    /// `api_key_index` is the API key index.
    pub fn new_with_account(base_url: &str, account_index: i64, api_key_index: i32) -> Self {
        use reqwest::header;
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            header::HeaderValue::from_static("gzip, deflate"),
        );

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .pool_max_idle_per_host(5)
            .default_headers(headers)
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            account_index,
            api_key_index,
            nonce: AtomicI64::new(0),
        }
    }

    /// Backward-compatible constructor matching the old 4-arg signature.
    /// The api_key and secret_key params are ignored (signing is done via FFI).
    pub fn new(
        _api_key: &str,
        _secret_key: &str,
        rest_url: &str,
        _ws_url: &str,
    ) -> Self {
        Self::new_with_account(rest_url, 0, 0)
    }

    /// HTTP GET with automatic retry on transient errors (502, 503, 504, timeouts)
    async fn http_get_json(&self, url: &str) -> Result<Value, LighterError> {
        let max_retries = 3u32;
        let mut delay = 2u64;
        for attempt in 0..=max_retries {
            match self.client.get(url).send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_server_error() && attempt < max_retries {
                        warn!("HTTP {} on GET {}, retry {}/{}...", status, url, attempt + 1, max_retries);
                        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                        delay = (delay * 2).min(10);
                        continue;
                    }
                    let body = resp.text().await.map_err(|e| LighterError::ApiError {
                        code: -1,
                        message: format!("Response body read error: {}", e),
                    })?;
                    let json: Value = serde_json::from_str(&body).map_err(|e| LighterError::ApiError {
                        code: status.as_u16() as i32,
                        message: format!("JSON parse error (HTTP {}): {} — body: {}", status, e, &body[..body.len().min(200)]),
                    })?;
                    return Ok(json);
                }
                Err(e) if attempt < max_retries => {
                    warn!("HTTP request error on GET {}: {}, retry {}/{}...", url, e, attempt + 1, max_retries);
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                    delay = (delay * 2).min(10);
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!()
    }

    /// Fetch and cache the nonce from the API
    pub async fn refresh_nonce(&self) -> Result<i64, LighterError> {
        let url = format!(
            "{}/api/v1/nextNonce?account_index={}&api_key_index={}",
            self.base_url, self.account_index, self.api_key_index
        );
        debug!("GET {}", url);

        let resp: Value = self.http_get_json(&url).await?;
        let nonce = resp["nonce"]
            .as_str()
            .and_then(|s| s.parse::<i64>().ok())
            .or_else(|| resp["nonce"].as_i64())
            .or_else(|| resp["next_nonce"].as_str().and_then(|s| s.parse::<i64>().ok()))
            .or_else(|| resp["next_nonce"].as_i64())
            .ok_or_else(|| LighterError::ApiError {
                code: -1,
                message: format!("Invalid nonce response: {}", resp),
            })?;

        self.nonce.store(nonce, Ordering::SeqCst);
        Ok(nonce)
    }

    /// Get next nonce (fetch if not yet cached, otherwise increment locally)
    async fn next_nonce(&self) -> Result<i64, LighterError> {
        let current = self.nonce.load(Ordering::SeqCst);
        if current == 0 {
            self.refresh_nonce().await
        } else {
            Ok(self.nonce.fetch_add(1, Ordering::SeqCst))
        }
    }

    /// Get account info
    pub async fn get_account_info(&self) -> Result<AccountInfo, LighterError> {
        let url = format!(
            "{}/api/v1/account?by=index&value={}",
            self.base_url, self.account_index
        );
        debug!("GET {}", url);

        let resp: Value = self.http_get_json(&url).await?;

        // Try both "detailed_accounts" and "accounts" keys
        let account = resp["detailed_accounts"]
            .as_array()
            .and_then(|arr| arr.first())
            .or_else(|| resp["accounts"].as_array().and_then(|arr| arr.first()))
            .ok_or_else(|| LighterError::ApiError {
                code: -1,
                message: "No account found in response".into(),
            })?;

        let collateral = account["collateral"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| account["available_balance"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0.0);

        let free_balance = account["available_balance"]
            .as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(collateral);

        // Note: Lighter API has no "equity" field, falls back to collateral.
        // True equity is computed below as collateral + Σ(unrealized_pnl).

        let mut positions = Vec::new();
        if let Some(pos_arr) = account["positions"].as_array() {
            for p in pos_arr {
                // Position field can be "position" or "size"
                let size: f64 = p["position"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| p["size"].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0);
                // Apply sign field if present
                let sign: f64 = p["sign"]
                    .as_i64()
                    .map(|s| if s >= 0 { 1.0 } else { -1.0 })
                    .unwrap_or(1.0);
                let signed_size = size * sign;
                if signed_size.abs() < 1e-12 {
                    continue;
                }
                let side = if signed_size >= 0.0 { Side::Buy } else { Side::Sell };
                let entry_price: f64 = p["avg_entry_price"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| p["entry_price"].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0.0);
                let unrealized_pnl: f64 = p["unrealized_pnl"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);
                let market_index = p["market_id"]
                    .as_u64()
                    .or_else(|| p["market_id"].as_str().and_then(|s| s.parse().ok()))
                    .or_else(|| p["market_index"].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(0) as u32;

                let symbol = p["symbol"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| match market_index {
                        0 => "ETH".to_string(),
                        1 => "BTC".to_string(),
                        _ => format!("MARKET_{}", market_index),
                    });

                positions.push(Position {
                    symbol,
                    side,
                    size: signed_size.abs(),
                    entry_price,
                    unrealized_pnl,
                    leverage: 1.0,
                });
            }
        }

        // Lighter API has no separate "equity" field — `collateral` is the margin balance
        // which does NOT include unrealized PnL. True equity = collateral + Σ(unrealized_pnl).
        let total_unrealized: f64 = positions.iter().map(|p| p.unrealized_pnl).sum();
        let true_equity = collateral + total_unrealized;

        Ok(AccountInfo {
            balances: vec![Balance {
                asset: "USDC".into(),
                free: free_balance,
                locked: 0.0,
            }],
            positions,
            total_equity: true_equity,
        })
    }

    /// Get market info for a specific market
    #[allow(dead_code)]
    pub async fn get_market_info(&self, market_id: u32) -> Result<MarketInfo, LighterError> {
        let url = format!(
            "{}/api/v1/orderBookDetails?market_id={}",
            self.base_url, market_id
        );
        debug!("GET {}", url);

        let resp: Value = self.http_get_json(&url).await?;

        let details = resp["order_book_details"]
            .as_array()
            .and_then(|arr| arr.first())
            .ok_or_else(|| LighterError::ApiError {
                code: -1,
                message: format!("No market details for market_id={}", market_id),
            })?;

        let size_decimals = details["size_decimals"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| details["size_decimals"].as_u64().map(|v| v as u32))
            .unwrap_or(4);

        let price_decimals = details["price_decimals"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| details["price_decimals"].as_u64().map(|v| v as u32))
            .unwrap_or(2);

        let min_base: f64 = details["min_base_amount"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.001);

        let min_quote: f64 = details["min_quote_amount"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1.0);

        let last_price: f64 = details["last_trade_price"]
            .as_f64()
            .or_else(|| details["last_trade_price"].as_str().and_then(|s| s.parse().ok()))
            .unwrap_or(0.0);

        let symbol = details["symbol"]
            .as_str()
            .unwrap_or(match market_id {
                0 => "ETH",
                1 => "BTC",
                _ => "UNKNOWN",
            })
            .to_string();

        Ok(MarketInfo {
            market_id,
            symbol,
            size_decimals,
            price_decimals,
            min_base_amount: min_base,
            min_quote_amount: min_quote,
            last_trade_price: last_price,
        })
    }

    /// Get order book
    #[allow(dead_code)]
    pub async fn get_order_book(
        &self,
        market_id: u32,
        limit: u32,
    ) -> Result<OrderBook, LighterError> {
        let url = format!(
            "{}/api/v1/orderBookOrders?market_id={}&limit={}",
            self.base_url, market_id, limit
        );
        debug!("GET {}", url);

        let resp: Value = self.http_get_json(&url).await?;

        let symbol = match market_id {
            0 => "ETH".to_string(),
            1 => "BTC".to_string(),
            _ => format!("MARKET_{}", market_id),
        };

        let parse_levels = |key: &str| -> Vec<PriceLevel> {
            resp[key]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|lvl| {
                            let price = lvl["price"]
                                .as_str()
                                .and_then(|s| s.parse::<f64>().ok())?;
                            let qty = lvl["size"]
                                .as_str()
                                .and_then(|s| s.parse::<f64>().ok())
                                .unwrap_or(0.0);
                            Some(PriceLevel {
                                price,
                                quantity: qty,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        Ok(OrderBook {
            symbol,
            market_id,
            bids: parse_levels("bids"),
            asks: parse_levels("asks"),
            timestamp: chrono::Utc::now(),
        })
    }

    /// Get recent trades
    #[allow(dead_code)]
    pub async fn get_recent_trades(
        &self,
        market_id: u32,
        limit: u32,
    ) -> Result<Vec<Trade>, LighterError> {
        let url = format!(
            "{}/api/v1/recentTrades?market_id={}&limit={}",
            self.base_url, market_id, limit
        );
        debug!("GET {}", url);

        let resp: Value = self.http_get_json(&url).await?;

        let symbol = match market_id {
            0 => "ETH".to_string(),
            1 => "BTC".to_string(),
            _ => format!("MARKET_{}", market_id),
        };

        let trades = resp["trades"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .enumerate()
                    .filter_map(|(i, t)| {
                        let price: f64 =
                            t["price"].as_str().and_then(|s| s.parse().ok())?;
                        let qty: f64 = t["size"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                            .or_else(|| {
                                t["quantity"].as_str().and_then(|s| s.parse().ok())
                            })
                            .unwrap_or(0.0);
                        let side_str = t["side"].as_str().unwrap_or("buy");
                        let side = if side_str == "ask" || side_str == "sell" {
                            Side::Sell
                        } else {
                            Side::Buy
                        };
                        let ts = t["timestamp"]
                            .as_str()
                            .and_then(|s| s.parse::<i64>().ok())
                            .or_else(|| t["timestamp"].as_i64())
                            .unwrap_or(0);
                        let dt = chrono::DateTime::from_timestamp(ts, 0)
                            .unwrap_or_else(chrono::Utc::now);

                        Some(Trade {
                            id: t["trade_index"]
                                .as_str()
                                .unwrap_or(&i.to_string())
                                .to_string(),
                            symbol: symbol.clone(),
                            market_id,
                            price,
                            quantity: qty,
                            side,
                            timestamp: dt,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(trades)
    }

    /// Get candlestick data
    #[allow(dead_code)]
    pub async fn get_candlesticks(
        &self,
        market_id: u32,
        resolution: &str,
        count_back: u32,
    ) -> Result<Vec<Candlestick>, LighterError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let start = now - 86400 * 7; // 7 days back

        self.get_candlesticks_in_range(market_id, resolution, start as i64, now as i64, count_back)
            .await
    }

    pub async fn get_candlesticks_in_range(
        &self,
        market_id: u32,
        resolution: &str,
        start_timestamp: i64,
        end_timestamp: i64,
        count_back: u32,
    ) -> Result<Vec<Candlestick>, LighterError> {
        let url = format!(
            "{}/api/v1/candles?market_id={}&resolution={}&start_timestamp={}&end_timestamp={}&count_back={}",
            self.base_url, market_id, resolution, start_timestamp, end_timestamp, count_back
        );
        debug!("GET {}", url);

        let resp: Value = self.http_get_json(&url).await?;

        let symbol = match market_id {
            0 => "ETH".to_string(),
            1 => "BTC".to_string(),
            _ => format!("MARKET_{}", market_id),
        };

        let candles = resp["c"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let open = c["o"].as_f64()?;
                        let high = c["h"].as_f64()?;
                        let low = c["l"].as_f64()?;
                        let close = c["c"].as_f64()?;
                        let volume = c["v"].as_f64().unwrap_or(0.0);
                        // Timestamp is in milliseconds
                        let ts_ms = c["t"].as_i64()?;
                        let dt = chrono::DateTime::from_timestamp(ts_ms / 1000, 0)
                            .unwrap_or_else(chrono::Utc::now);

                        Some(Candlestick {
                            timestamp: dt,
                            open,
                            high,
                            low,
                            close,
                            volume,
                            symbol: symbol.clone(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(candles)
    }

    /// Get next nonce from the API
    #[allow(dead_code)]
    pub async fn get_next_nonce(&self) -> Result<i64, LighterError> {
        self.refresh_nonce().await
    }

    /// Send a signed transaction
    pub async fn send_tx(
        &self,
        tx_type: u8,
        tx_info: &str,
    ) -> Result<PlaceOrderResponse, LighterError> {
        let url = format!("{}/api/v1/sendTx", self.base_url);

        // API expects multipart/form-data with tx_type and tx_info as form fields
        let form = reqwest::multipart::Form::new()
            .text("tx_type", tx_type.to_string())
            .text("tx_info", tx_info.to_string());

        info!("sendTx: tx_type={}, tx_info_preview={}...",
            tx_type, &tx_info[..tx_info.len().min(200)]);

        let resp_raw = self
            .client
            .post(&url)
            .multipart(form)
            .send()
            .await?;

        let status = resp_raw.status();
        let resp_text = resp_raw.text().await.unwrap_or_default();
        debug!("sendTx response ({}): {}", status, &resp_text);

        if !status.is_success() {
            error!("sendTx FAILED ({}): {} — tx_type={}, tx_info={}", status, resp_text, tx_type, tx_info);
            return Err(LighterError::ApiError {
                code: status.as_u16() as i32,
                message: format!("HTTP {}: {}", status, resp_text),
            });
        }

        let resp: Value = serde_json::from_str(&resp_text).map_err(|e| {
            LighterError::ApiError {
                code: -1,
                message: format!("Failed to parse sendTx response: {} body={}", e, resp_text),
            }
        })?;

        // Check for API error
        if let Some(err_msg) = resp.get("error").and_then(|e| e.as_str()) {
            if !err_msg.is_empty() {
                return Err(LighterError::ApiError {
                    code: resp["code"].as_i64().unwrap_or(-1) as i32,
                    message: err_msg.to_string(),
                });
            }
        }

        let order_id = resp["tx_hash"]
            .as_str()
            .or_else(|| resp["hash"].as_str())
            .or_else(|| resp["txHash"].as_str())
            .unwrap_or("pending")
            .to_string();

        info!("sendTx success: id={} full_response={}", order_id, resp_text);

        Ok(PlaceOrderResponse {
            order_id,
            status: "submitted".into(),
        })
    }

    /// Place an order using FFI signing
    pub async fn place_order(
        &self,
        _symbol: &str,
        side: Side,
        price: f64,
        quantity: f64,
    ) -> Result<PlaceOrderResponse, LighterError> {
        self.place_order_with_market(0, side, price, quantity, None)
            .await
    }

    /// Place an order for a specific market with optional MarketInfo.
    /// Automatically enforces minimum base/quote amounts.
    pub async fn place_order_with_market(
        &self,
        market_id: u32,
        side: Side,
        price: f64,
        quantity: f64,
        market_info: Option<&MarketInfo>,
    ) -> Result<PlaceOrderResponse, LighterError> {
        let (size_dec, price_dec, min_base, min_quote) = match market_info {
            Some(mi) => (mi.size_decimals, mi.price_decimals, mi.min_base_amount, mi.min_quote_amount),
            None => match market_id {
                0 => (4, 2, 0.005, 10.0),   // ETH
                1 => (5, 1, 0.0002, 10.0),  // BTC
                _ => (4, 2, 0.005, 10.0),
            },
        };

        // Enforce minimum quantity: must meet both min_base and min_quote
        let mut qty = quantity;
        if qty < min_base {
            info!("Adjusting qty from {:.6} to min_base {:.6} for market {}", qty, min_base, market_id);
            qty = min_base;
        }
        let quote_value = qty * price;
        if quote_value < min_quote && price > 0.0 {
            qty = min_quote / price * 1.02; // 2% buffer for rounding
            info!("Adjusting qty to {:.6} to meet min_quote ${:.2} for market {}", qty, min_quote, market_id);
        }

        let size_multiplier = 10_f64.powi(size_dec as i32);
        let price_multiplier = 10_f64.powi(price_dec as i32);
        let mut base_amount = (qty * size_multiplier).round() as i64;
        let price_int = (price * price_multiplier).round() as i32;

        // Post-rounding check: ensure integer base_amount * price meets min_quote
        let actual_quote = (base_amount as f64 / size_multiplier) * price;
        if actual_quote < min_quote && price > 0.0 {
            // Compute minimum base_amount that meets min_quote
            let min_base_for_quote = (min_quote / price * size_multiplier).ceil() as i64 + 1;
            info!("Post-rounding fix: base_amount {} -> {} to meet min_quote ${:.2}", base_amount, min_base_for_quote, min_quote);
            base_amount = min_base_for_quote;
        }

        // Also ensure base_amount meets min_base in integer form
        let min_base_int = (min_base * size_multiplier).ceil() as i64;
        if base_amount < min_base_int {
            info!("Post-rounding fix: base_amount {} -> min_base_int {}", base_amount, min_base_int);
            base_amount = min_base_int;
        }

        let is_ask = matches!(side, Side::Sell);

        let nonce = self.next_nonce().await?;
        info!(
            "Placing order: market={} side={:?} price={} qty={} (base_amount={} price_int={} nonce={})",
            market_id, side, price, quantity, base_amount, price_int, nonce
        );

        let (tx_type, tx_info) = ffi::sign_create_order(
            market_id as i32,
            base_amount,
            price_int,
            is_ask,
            0, // Limit
            1, // GoodTillTime
            nonce,
        )?;

        match self.send_tx(tx_type, &tx_info).await {
            Ok(resp) => Ok(resp),
            Err(e) => {
                // Reset nonce from server on failure
                let _ = self.refresh_nonce().await;
                Err(e)
            }
        }
    }

    /// Cancel an order
    pub async fn cancel_order_by_index(
        &self,
        market_id: u32,
        order_index: i64,
    ) -> Result<(), LighterError> {
        let nonce = self.next_nonce().await?;
        info!(
            "Cancelling order: market={} order_index={} nonce={}",
            market_id, order_index, nonce
        );

        let (tx_type, tx_info) =
            ffi::sign_cancel_order(market_id as i32, order_index, nonce)?;

        self.send_tx(tx_type, &tx_info).await?;
        Ok(())
    }

    /// Cancel order by string ID (backward compat — parses as order_index)
    #[allow(dead_code)]
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), LighterError> {
        let order_index: i64 = order_id.parse().map_err(|_| LighterError::ApiError {
            code: -1,
            message: format!("Invalid order_id for cancel: {}", order_id),
        })?;
        self.cancel_order_by_index(0, order_index).await
    }

    /// Get open orders for the account
    /// Get open (active) orders from the exchange.
    /// Uses `/api/v1/accountActiveOrders` per-market (official Lighter API).
    pub async fn get_open_orders(&self, _symbol: &str) -> Result<Vec<Order>, LighterError> {
        let mut all_orders = Vec::new();

        // Create auth token (valid for 60 seconds)
        let deadline = chrono::Utc::now().timestamp() + 60;
        let auth_token = ffi::create_auth_token(deadline)?;

        // Query each market (0=ETH, 1=BTC)
        for market_id in [0u32, 1u32] {
            let url = format!(
                "{}/api/v1/accountActiveOrders?account_index={}&market_id={}&auth={}",
                self.base_url, self.account_index, market_id, auth_token
            );

            let resp = match self.client.get(&url).send().await {
                Ok(r) => {
                    // Retry once on server errors (502/503/504)
                    if r.status().is_server_error() {
                        warn!("get_open_orders market {}: HTTP {}, retrying...", market_id, r.status());
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        match self.client.get(&url).send().await {
                            Ok(r2) => r2,
                            Err(e) => {
                                warn!("get_open_orders retry failed for market {}: {}", market_id, e);
                                continue;
                            }
                        }
                    } else {
                        r
                    }
                }
                Err(e) => {
                    warn!("get_open_orders request failed for market {}: {}", market_id, e);
                    continue;
                }
            };

            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();

            if !status.is_success() {
                warn!("get_open_orders market {}: HTTP {} — {}", market_id, status, &body[..body.len().min(200)]);
                continue;
            }

            let json: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    warn!("get_open_orders market {}: JSON parse error: {}", market_id, e);
                    continue;
                }
            };

            if let Some(order_arr) = json.get("orders").and_then(|o| o.as_array()) {
                let market_name = if market_id == 0 { "ETH" } else { "BTC" };
                for o in order_arr {
                    let is_ask = o["is_ask"].as_str()
                        .or_else(|| o["is_ask"].as_bool().map(|b| if b { "1" } else { "0" }).or(Some("0")))
                        .unwrap_or("0");
                    let side = if is_ask == "1" || is_ask == "true" { Side::Sell } else { Side::Buy };

                    let price_val = o["price"].as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .or_else(|| o["price"].as_f64())
                        .unwrap_or(0.0);
                    let remaining = o["remaining_base_amount"].as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .or_else(|| o["remaining_base_amount"].as_f64())
                        .unwrap_or(0.0);
                    let original = o["original_base_amount"].as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .or_else(|| o["original_base_amount"].as_f64())
                        .unwrap_or(remaining);

                    let status = if remaining < original && remaining > 0.0 {
                        OrderStatus::PartiallyFilled
                    } else {
                        OrderStatus::New
                    };

                    all_orders.push(Order {
                        id: o["order_index"].as_str()
                            .unwrap_or(&o["order_index"].to_string())
                            .to_string(),
                        symbol: market_name.to_string(),
                        side,
                        price: price_val,
                        quantity: original,
                        filled_quantity: original - remaining,
                        status,
                        order_type: OrderType::Limit,
                        created_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    });
                }
            }
        }

        Ok(all_orders)
    }

    /// Cancel all orders (stub for backward compat)
    #[allow(dead_code)]
    pub async fn cancel_all_orders(&self, _symbol: &str) -> Result<(), LighterError> {
        let nonce = self.next_nonce().await?;
        let (tx_type, tx_info) = ffi::sign_cancel_all_orders(nonce)?;
        self.send_tx(tx_type, &tx_info).await?;
        Ok(())
    }

    /// Get positions (via account info)
    #[allow(dead_code)]
    pub async fn get_positions(&self) -> Result<Vec<Position>, LighterError> {
        let info = self.get_account_info().await?;
        Ok(info.positions)
    }
}
