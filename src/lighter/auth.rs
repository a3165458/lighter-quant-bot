use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// 生成HMAC-SHA256签名
pub fn sign_request(secret_key: &str, message: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes())
        .expect("HMAC接受任意长度密钥");
    mac.update(message.as_bytes());
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// 构建签名消息体（使用预分配缓冲区减少分配）
pub fn build_sign_message(timestamp: u64, method: &str, path: &str, body: &str) -> String {
    let mut msg = String::with_capacity(20 + method.len() + path.len() + body.len());
    use std::fmt::Write;
    let _ = write!(msg, "{}{}{}{}", timestamp, method, path, body);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sign_request() {
        let secret = "test_secret";
        let message = "test_message";
        let signature = sign_request(secret, message);
        assert!(!signature.is_empty());
        // 签名应为64个十六进制字符
        assert_eq!(signature.len(), 64);
    }

    #[test]
    fn test_build_sign_message() {
        let msg = build_sign_message(1234567890, "GET", "/api/v1/account", "");
        assert_eq!(msg, "1234567890GET/api/v1/account");
    }
}
