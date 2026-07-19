use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

/// 全局市场符号注册表 (market_id <-> symbol)
///
/// 默认内置主网 ETH/BTC 映射；启动时应调用 `register_all` 用交易所
/// 实际返回的市场列表覆盖（支持 Robinhood Chain 实例的股票永续等市场）。
fn store() -> &'static RwLock<HashMap<u32, String>> {
    static STORE: OnceLock<RwLock<HashMap<u32, String>>> = OnceLock::new();
    STORE.get_or_init(|| {
        RwLock::new(HashMap::from([
            (0u32, "ETH".to_string()),
            (1u32, "BTC".to_string()),
        ]))
    })
}

/// 注册市场映射（增量合并，后注册覆盖先注册）
pub fn register_all(entries: impl IntoIterator<Item = (u32, String)>) {
    let mut map = store().write().unwrap();
    for (id, sym) in entries {
        map.insert(id, sym);
    }
}

/// market_id -> symbol，未知市场返回 "MARKET_{id}"
pub fn symbol_of(market_id: u32) -> String {
    store()
        .read()
        .unwrap()
        .get(&market_id)
        .cloned()
        .unwrap_or_else(|| format!("MARKET_{}", market_id))
}

/// symbol -> market_id（大小写不敏感；兼容 "MARKET_{id}" 形式）
pub fn market_id_of(symbol: &str) -> Option<u32> {
    let upper = symbol.to_ascii_uppercase();
    if let Some(rest) = upper.strip_prefix("MARKET_") {
        if let Ok(id) = rest.parse() {
            return Some(id);
        }
    }
    store()
        .read()
        .unwrap()
        .iter()
        .find(|(_, s)| s.to_ascii_uppercase() == upper)
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_mainnet_map() {
        assert_eq!(symbol_of(0), "ETH");
        assert_eq!(symbol_of(1), "BTC");
        assert_eq!(market_id_of("btc"), Some(1));
    }

    #[test]
    fn test_register_and_lookup() {
        register_all([(16u32, "TSLA".to_string())]);
        assert_eq!(symbol_of(16), "TSLA");
        assert_eq!(market_id_of("TSLA"), Some(16));
        assert_eq!(market_id_of("MARKET_99"), Some(99));
        assert!(symbol_of(98765).starts_with("MARKET_"));
    }
}
