use serde_json::Value;

use super::error::LighterError;
use super::types::{AccountInfo, Balance, Position, Side};

/// Parse a Lighter / Robinhood `/api/v1/account` JSON payload.
///
/// Official shape is `{ "code": 200, "accounts": [ DetailedAccount, ... ] }`.
/// RH and wrappers also show up as `detailed_accounts`, `data.accounts`,
/// a singular `account` object, or numeric-string balances. Error bodies
/// like `{ "code": 29404, "message": "not found" }` have no accounts list.
pub fn parse_account_info(resp: &Value) -> Result<AccountInfo, LighterError> {
    if let Some(account) = first_account_object(resp) {
        return Ok(account_from_json(account));
    }

    let code = json_i64(resp.get("code")).unwrap_or(-1) as i32;
    let message = resp
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if accounts_list_present_and_empty(resp) || code == 200 {
        return Ok(empty_account_info());
    }

    Err(LighterError::ApiError {
        code,
        message: if message.is_empty() {
            format!("No account found in response: {}", truncate_json(resp))
        } else {
            message
        },
    })
}

pub fn empty_account_info() -> AccountInfo {
    AccountInfo {
        balances: vec![Balance {
            asset: "USDG".into(),
            free: 0.0,
            locked: 0.0,
        }],
        positions: Vec::new(),
        total_equity: 0.0,
    }
}

pub fn account_looks_empty(info: &AccountInfo) -> bool {
    info.positions.is_empty()
        && info.total_equity.abs() < 1e-12
        && info
            .balances
            .iter()
            .all(|balance| balance.free.abs() < 1e-12)
}

fn first_account_object(resp: &Value) -> Option<&Value> {
    const KEYS: [&str; 4] = [
        "accounts",
        "detailed_accounts",
        "account",
        "detailed_account",
    ];
    let nested = [resp.get("data"), resp.get("result"), resp.get("payload")];
    let roots = std::iter::once(Some(resp)).chain(nested);

    for root in roots.flatten() {
        for key in KEYS {
            let Some(node) = root.get(key) else {
                continue;
            };
            if let Some(first) = node
                .as_array()
                .and_then(|arr| arr.iter().find(|v| v.is_object()))
            {
                return Some(first);
            }
            if node.is_object() && looks_like_account(node) {
                return Some(node);
            }
            if let Some(first) = node.as_object().and_then(|map| {
                map.values()
                    .find(|value| value.is_object() && looks_like_account(value))
            }) {
                return Some(first);
            }
        }
        if looks_like_account(root)
            && root.get("accounts").is_none()
            && root.get("detailed_accounts").is_none()
        {
            return Some(root);
        }
    }
    None
}

fn looks_like_account(value: &Value) -> bool {
    value.is_object()
        && (value.get("collateral").is_some()
            || value.get("available_balance").is_some()
            || value.get("positions").is_some()
            || value.get("account_index").is_some()
            || value.get("index").is_some())
}

fn accounts_list_present_and_empty(resp: &Value) -> bool {
    ["accounts", "detailed_accounts"].iter().any(|key| {
        resp.get(*key)
            .and_then(|value| value.as_array())
            .is_some_and(|arr| arr.is_empty())
            || resp
                .get("data")
                .and_then(|data| data.get(*key))
                .and_then(|value| value.as_array())
                .is_some_and(|arr| arr.is_empty())
    })
}

fn account_from_json(account: &Value) -> AccountInfo {
    let collateral = json_f64(account.get("collateral"))
        .or_else(|| json_f64(account.get("available_balance")))
        .or_else(|| json_f64(account.get("total_asset_value")))
        .unwrap_or(0.0);
    let free_balance = json_f64(account.get("available_balance")).unwrap_or(collateral);
    let asset = quote_asset(account);

    let mut positions = Vec::new();
    if let Some(pos_arr) = account.get("positions").and_then(|value| value.as_array()) {
        for position in pos_arr {
            let size = json_f64(position.get("position"))
                .or_else(|| json_f64(position.get("size")))
                .unwrap_or(0.0);
            let sign = json_f64(position.get("sign"))
                .map(|value| if value >= 0.0 { 1.0 } else { -1.0 })
                .unwrap_or(1.0);
            let signed_size = size * sign;
            if signed_size.abs() < 1e-12 {
                continue;
            }
            let side = if signed_size >= 0.0 {
                Side::Buy
            } else {
                Side::Sell
            };
            let entry_price = json_f64(position.get("avg_entry_price"))
                .or_else(|| json_f64(position.get("entry_price")))
                .unwrap_or(0.0);
            let unrealized_pnl = json_f64(position.get("unrealized_pnl")).unwrap_or(0.0);
            let market_index = json_f64(position.get("market_id"))
                .or_else(|| json_f64(position.get("market_index")))
                .unwrap_or(0.0) as u32;
            let symbol = position
                .get("symbol")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string())
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

    let total_unrealized: f64 = positions
        .iter()
        .map(|position| position.unrealized_pnl)
        .sum();
    AccountInfo {
        balances: vec![Balance {
            asset,
            free: free_balance,
            locked: 0.0,
        }],
        total_equity: collateral + total_unrealized,
        positions,
    }
}

fn quote_asset(account: &Value) -> String {
    account
        .get("assets")
        .and_then(|value| value.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let symbol = asset.get("symbol").and_then(|value| value.as_str())?;
                matches!(symbol, "USDG" | "USDC").then(|| symbol.to_string())
            })
        })
        .unwrap_or_else(|| "USDG".to_string())
}

fn json_f64(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|n| n as f64))
        .or_else(|| value.as_u64().map(|n| n as f64))
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|n| n as i64))
        .or_else(|| value.as_f64().map(|n| n as i64))
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
}

fn truncate_json(value: &Value) -> String {
    let raw = value.to_string();
    if raw.len() <= 200 {
        raw
    } else {
        format!("{}…", &raw[..200])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_official_rh_accounts_array_with_string_and_numeric_fields() {
        let resp = json!({
            "code": 200,
            "total": 1,
            "accounts": [{
                "code": 0,
                "index": 42,
                "account_index": 42,
                "collateral": "1295.50",
                "available_balance": 1200,
                "assets": [{"symbol": "USDG", "asset_id": 3, "balance": "0"}],
                "positions": [{
                    "market_id": 1,
                    "symbol": "BTC",
                    "sign": "1",
                    "position": "0.000866",
                    "avg_entry_price": 69253.9,
                    "unrealized_pnl": "2.5"
                }]
            }]
        });
        let info = parse_account_info(&resp).expect("valid RH account");
        assert!((info.total_equity - 1298.0).abs() < 1e-9);
        assert_eq!(info.balances[0].asset, "USDG");
        assert!((info.balances[0].free - 1200.0).abs() < 1e-9);
        assert_eq!(info.positions.len(), 1);
        assert_eq!(info.positions[0].symbol, "BTC");
        assert_eq!(info.positions[0].side, Side::Buy);
        assert!((info.positions[0].size - 0.000866).abs() < 1e-12);
    }

    #[test]
    fn parses_wrapped_and_singular_account_objects() {
        let wrapped = json!({
            "data": {
                "detailed_accounts": [{
                    "collateral": 10.0,
                    "available_balance": "10",
                    "positions": []
                }]
            }
        });
        let info = parse_account_info(&wrapped).expect("wrapped account");
        assert!((info.total_equity - 10.0).abs() < 1e-12);

        let singular = json!({
            "account": {
                "collateral": "5",
                "positions": []
            }
        });
        let info = parse_account_info(&singular).expect("singular account");
        assert!((info.total_equity - 5.0).abs() < 1e-12);
    }

    #[test]
    fn empty_success_list_is_empty_account_not_an_error() {
        let resp = json!({ "code": 200, "total": 0, "accounts": [] });
        let info = parse_account_info(&resp).expect("empty list");
        assert!(account_looks_empty(&info));
    }

    #[test]
    fn api_error_body_surfaces_exchange_message_instead_of_generic_missing() {
        let resp = json!({ "code": 29404, "message": "not found" });
        let err = parse_account_info(&resp).expect_err("error body");
        match err {
            LighterError::ApiError { code, message } => {
                assert_eq!(code, 29404);
                assert_eq!(message, "not found");
                assert!(!message.contains("No account found in response"));
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
