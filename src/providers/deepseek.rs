use crate::cache::CacheStore;
use crate::model::{AccountBalance, Provider, ProviderSnapshot};
use crate::providers::ProviderError;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
const AUTH_PROVIDER_KEY: &str = "deepseek";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekCredentials {
    pub key: String,
}

pub fn fetch() -> Result<ProviderSnapshot> {
    let path = auth_path().context("resolve DeepSeek auth path")?;
    let credentials = read_credentials(&path).map_err(anyhow::Error::from)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let response = agent
        .get(BALANCE_URL)
        .set("Authorization", &format!("Bearer {}", credentials.key))
        .set("Accept", "application/json")
        .call();
    match response {
        Ok(response) => {
            let value: Value = response
                .into_json()
                .context("decode DeepSeek balance response")?;
            parse_balance_response(&value, CacheStore::now_unix()).map_err(anyhow::Error::from)
        }
        Err(ureq::Error::Status(code, response)) => {
            let body: Value = response.into_json().unwrap_or(Value::Null);
            Err(anyhow::Error::new(error_from_status(code, &body)))
        }
        Err(error) => Err(anyhow::Error::new(ProviderError::Request(
            http_error_status(&error),
        ))),
    }
}

pub fn auth_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".pi")
        .join("agent")
        .join("auth.json"))
}

pub fn read_credentials(path: &Path) -> std::result::Result<DeepSeekCredentials, ProviderError> {
    let bytes = fs::read(path).map_err(|_| ProviderError::MissingCredentials)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        ProviderError::Unavailable("DeepSeek auth file is not valid JSON".to_string())
    })?;
    find_key(&value).ok_or(ProviderError::MissingCredentials)
}

fn find_key(value: &Value) -> Option<DeepSeekCredentials> {
    let key = value
        .get(AUTH_PROVIDER_KEY)?
        .get("key")?
        .as_str()
        .filter(|key| !key.trim().is_empty())?;
    Some(DeepSeekCredentials {
        key: key.to_string(),
    })
}

pub fn parse_balance_response(
    value: &Value,
    fetched_at_unix: u64,
) -> std::result::Result<ProviderSnapshot, ProviderError> {
    let is_available = value
        .get("is_available")
        .and_then(Value::as_bool)
        .ok_or_else(|| ProviderError::UnsupportedResponse("missing is_available".to_string()))?;
    let balances = value
        .get("balance_infos")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::UnsupportedResponse("missing balance_infos".to_string()))?;
    let balance = balances
        .iter()
        .filter_map(parse_balance)
        .find(|balance| balance.currency == "USD")
        .or_else(|| balances.iter().filter_map(parse_balance).next())
        .ok_or_else(|| {
            ProviderError::UnsupportedResponse("balance_infos has no supported balance".to_string())
        })?;
    Ok(
        ProviderSnapshot::new(Provider::DeepSeek, vec![], fetched_at_unix).with_balance(Some(
            AccountBalance {
                currency: balance.currency,
                total_balance: balance.total_balance,
                is_available,
            },
        )),
    )
}

fn parse_balance(value: &Value) -> Option<AccountBalanceInfo> {
    let currency = value.get("currency").and_then(Value::as_str)?;
    if !matches!(currency, "CNY" | "USD") {
        return None;
    }
    let total_balance = value
        .get("total_balance")
        .and_then(Value::as_str)
        .filter(|balance| !balance.trim().is_empty())?;
    Some(AccountBalanceInfo {
        currency: currency.to_string(),
        total_balance: total_balance.to_string(),
    })
}

#[derive(Debug)]
struct AccountBalanceInfo {
    currency: String,
    total_balance: String,
}

pub fn error_from_status(status: u16, body: &Value) -> ProviderError {
    let detail = body
        .pointer("/error/message")
        .or_else(|| body.pointer("/error/type"))
        .and_then(Value::as_str);
    match status {
        401 => ProviderError::MissingCredentials,
        403 => ProviderError::Unavailable(
            detail
                .unwrap_or("DeepSeek balance access is unavailable")
                .to_string(),
        ),
        _ => ProviderError::Request(format!("HTTP {status}")),
    }
}

fn http_error_status(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        ureq::Error::Transport(error) => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn reads_the_deepseek_entry_from_top_level_provider_keys() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("auth.json");
        fs::write(
            &path,
            r#"{"openai":{"type":"api","key":"other"},"deepseek":{"type":"api","key":"unit-test-key"},"xai":{}}"#,
        )
        .unwrap();
        assert_eq!(
            read_credentials(&path).unwrap(),
            DeepSeekCredentials {
                key: "unit-test-key".to_string()
            }
        );
    }

    #[test]
    fn missing_auth_file_or_key_is_unavailable() {
        let directory = tempdir().unwrap();
        assert_eq!(
            read_credentials(&directory.path().join("missing.json"))
                .unwrap_err()
                .to_string(),
            "provider credentials are unavailable"
        );
        let path = directory.path().join("auth.json");
        fs::write(&path, r#"{"xai":{"type":"api"}}"#).unwrap();
        assert_eq!(
            read_credentials(&path).unwrap_err().to_string(),
            "provider credentials are unavailable"
        );
        fs::write(&path, r#"{"deepseek":{"type":"api","key":"   "}}"#).unwrap();
        assert_eq!(
            read_credentials(&path).unwrap_err().to_string(),
            "provider credentials are unavailable"
        );
    }

    #[test]
    fn parses_usd_before_cny_without_converting_the_raw_balance() {
        let snapshot = parse_balance_response(
            &json!({
                "is_available": true,
                "balance_infos": [
                    {"currency":"CNY","total_balance":"28.90","granted_balance":"0.00","topped_up_balance":"28.90"},
                    {"currency":"USD","total_balance":"4.07","granted_balance":"0.00","topped_up_balance":"4.07"}
                ]
            }),
            1,
        )
        .unwrap();
        assert_eq!(snapshot.windows, Vec::new());
        assert_eq!(
            snapshot.balance.unwrap(),
            AccountBalance {
                currency: "USD".to_string(),
                total_balance: "4.07".to_string(),
                is_available: true,
            }
        );
    }

    #[test]
    fn cny_is_supported_when_it_is_the_only_balance() {
        let snapshot = parse_balance_response(
            &json!({
                "is_available": false,
                "balance_infos": [{"currency":"CNY","total_balance":"28.90"}]
            }),
            1,
        )
        .unwrap();
        assert_eq!(snapshot.balance.unwrap().currency, "CNY");
    }

    #[test]
    fn auth_and_entitlement_errors_are_unavailable_never_zero() {
        assert!(matches!(
            error_from_status(401, &json!({"type":"error"})),
            ProviderError::MissingCredentials
        ));
        let entitlement = error_from_status(
            403,
            &json!({"error":{"type":"EntitlementError","message":"balance unavailable"}}),
        );
        assert_eq!(
            entitlement.to_string(),
            "provider quota is unavailable: balance unavailable"
        );
        assert!(matches!(
            error_from_status(500, &Value::Null),
            ProviderError::Request(message) if message == "HTTP 500"
        ));
    }
}
