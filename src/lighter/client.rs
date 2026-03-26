use anyhow::Result;
use reqwest::Client;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};

use super::auth;
use super::error::LighterError;
use super::types::*;

/// Lighter交易所REST API客户端
pub struct LighterClient {
    client: Client,
    api_key: String,
    secret_key: String,
    rest_url: String,
    _ws_url: String,
}

impl LighterClient {
    pub fn new(api_key: &str, secret_key: &str, rest_url: &str, ws_url: &str) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .connect_timeout(std::time::Duration::from_secs(5))
            .pool_max_idle_per_host(5)
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            api_key: api_key.to_string(),
            secret_key: secret_key.to_string(),
            rest_url: rest_url.to_string(),
            _ws_url: ws_url.to_string(),
        }
    }

    /// 获取当前时间戳（毫秒）
    fn timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// 发送已签名的GET请求
    async fn signed_get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, LighterError> {
        let timestamp = Self::timestamp_ms();
        let sign_msg = auth::build_sign_message(timestamp, "GET", path, "");
        let signature = auth::sign_request(&self.secret_key, &sign_msg);

        let url = format!("{}{}", self.rest_url, path);
        debug!("GET {}", url);

        let response = self.client
            .get(&url)
            .header("X-API-KEY", &self.api_key)
            .header("X-TIMESTAMP", timestamp.to_string())
            .header("X-SIGNATURE", &signature)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16() as i32;
            let body = response.text().await.unwrap_or_default();
            return Err(LighterError::ApiError {
                code: status,
                message: body,
            });
        }

        let result = response.json::<T>().await?;
        Ok(result)
    }

    /// 发送已签名的POST请求
    async fn signed_post<T: serde::de::DeserializeOwned, B: serde::Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, LighterError> {
        let timestamp = Self::timestamp_ms();
        let body_str = serde_json::to_string(body)?;
        let sign_msg = auth::build_sign_message(timestamp, "POST", path, &body_str);
        let signature = auth::sign_request(&self.secret_key, &sign_msg);

        let url = format!("{}{}", self.rest_url, path);
        debug!("POST {}", url);

        let response = self.client
            .post(&url)
            .header("X-API-KEY", &self.api_key)
            .header("X-TIMESTAMP", timestamp.to_string())
            .header("X-SIGNATURE", &signature)
            .header("Content-Type", "application/json")
            .body(body_str)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16() as i32;
            let body = response.text().await.unwrap_or_default();
            return Err(LighterError::ApiError {
                code: status,
                message: body,
            });
        }

        let result = response.json::<T>().await?;
        Ok(result)
    }

    /// 获取账户信息
    pub async fn get_account_info(&self) -> Result<AccountInfo, LighterError> {
        self.signed_get("/account").await
    }

    /// 获取订单列表
    #[allow(dead_code)]
    pub async fn get_open_orders(&self, symbol: &str) -> Result<Vec<Order>, LighterError> {
        let path = format!("/orders?symbol={}", symbol);
        self.signed_get(&path).await
    }

    /// 下单
    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        price: f64,
        quantity: f64,
    ) -> Result<PlaceOrderResponse, LighterError> {
        let order = PlaceOrderRequest {
            symbol: symbol.to_string(),
            side,
            order_type: OrderType::Limit,
            price,
            quantity,
        };
        self.signed_post("/orders", &order).await
    }

    /// 取消订单
    #[allow(dead_code)]
    pub async fn cancel_order(&self, order_id: &str) -> Result<(), LighterError> {
        let path = format!("/orders/{}", order_id);
        let timestamp = Self::timestamp_ms();
        let sign_msg = auth::build_sign_message(timestamp, "DELETE", &path, "");
        let signature = auth::sign_request(&self.secret_key, &sign_msg);

        let url = format!("{}{}", self.rest_url, path);
        debug!("DELETE {}", url);

        let response = self.client
            .delete(&url)
            .header("X-API-KEY", &self.api_key)
            .header("X-TIMESTAMP", timestamp.to_string())
            .header("X-SIGNATURE", &signature)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16() as i32;
            let body = response.text().await.unwrap_or_default();
            return Err(LighterError::ApiError {
                code: status,
                message: body,
            });
        }

        Ok(())
    }

    /// 获取订单簿
    #[allow(dead_code)]
    pub async fn get_order_book(&self, symbol: &str, depth: u32) -> Result<OrderBook, LighterError> {
        let path = format!("/orderbook?symbol={}&depth={}", symbol, depth);
        self.signed_get(&path).await
    }

    /// 获取最近成交
    #[allow(dead_code)]
    pub async fn get_recent_trades(&self, symbol: &str, limit: u32) -> Result<Vec<Trade>, LighterError> {
        let path = format!("/trades?symbol={}&limit={}", symbol, limit);
        self.signed_get(&path).await
    }

    /// 获取K线数据
    #[allow(dead_code)]
    pub async fn get_candlesticks(
        &self,
        symbol: &str,
        interval: &str,
        limit: u32,
    ) -> Result<Vec<Candlestick>, LighterError> {
        let path = format!("/klines?symbol={}&interval={}&limit={}", symbol, interval, limit);
        self.signed_get(&path).await
    }

    /// 获取仓位信息
    #[allow(dead_code)]
    pub async fn get_positions(&self) -> Result<Vec<Position>, LighterError> {
        self.signed_get("/positions").await
    }

    /// 取消所有订单
    #[allow(dead_code)]
    pub async fn cancel_all_orders(&self, symbol: &str) -> Result<(), LighterError> {
        let path = format!("/orders/all?symbol={}", symbol);
        let timestamp = Self::timestamp_ms();
        let sign_msg = auth::build_sign_message(timestamp, "DELETE", &path, "");
        let signature = auth::sign_request(&self.secret_key, &sign_msg);

        let url = format!("{}{}", self.rest_url, path);
        warn!("取消所有订单: {}", symbol);

        let response = self.client
            .delete(&url)
            .header("X-API-KEY", &self.api_key)
            .header("X-TIMESTAMP", timestamp.to_string())
            .header("X-SIGNATURE", &signature)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16() as i32;
            let body = response.text().await.unwrap_or_default();
            return Err(LighterError::ApiError {
                code: status,
                message: body,
            });
        }

        Ok(())
    }
}
