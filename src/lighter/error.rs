use thiserror::Error;

#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum LighterError {
    #[error("HTTP请求失败: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("WebSocket错误: {0}")]
    WebSocketError(String),

    #[error("认证失败: {0}")]
    AuthError(String),

    #[error("API错误: {code} - {message}")]
    ApiError { code: i32, message: String },

    #[error("JSON解析失败: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("FFI错误: {0}")]
    FfiError(String),

    #[error("连接超时")]
    Timeout,

    #[error("连接断开")]
    Disconnected,

    #[error("订单被拒绝: {0}")]
    OrderRejected(String),

    #[error("余额不足: 需要 {required}, 可用 {available}")]
    InsufficientBalance { required: f64, available: f64 },

    #[error("频率限制: 请等待 {wait_ms}ms")]
    RateLimited { wait_ms: u64 },

    #[error("内部错误: {0}")]
    Internal(String),
}
