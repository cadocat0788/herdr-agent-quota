use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind};
use crate::providers::ProviderError;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
// auth.json stores every provider under top-level keys; only this one holds
// the OpenCode Go subscription key.
const AUTH_PROVIDER_KEY: &str = "opencode-go";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeCredentials {
    pub key: String,
}

pub fn fetch() -> Result<ProviderSnapshot> {
    let path = auth_path().context("resolve OpenCode auth path")?;
    let credentials = read_credentials(&path).map_err(anyhow::Error::from)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let response = agent
        .get(USAGE_URL)
        .set("Authorization", &format!("Bearer {}", credentials.key))
        .set("Accept", "application/json")
        .call();
    // Auth and entitlement failures carry a structured JSON body; surface the
    // contract's error type instead of a bare HTTP code. Credentials never
    // appear in any surfaced message.
    match response {
        Ok(response) => {
            let value: Value = response
                .into_json()
                .context("decode OpenCode usage response")?;
            parse_usage_response(&value, CacheStore::now_unix()).map_err(anyhow::Error::from)
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

/// `$XDG_DATA_HOME/opencode/auth.json`, else `~/.local/share/opencode/auth.json`.
pub fn auth_path() -> Result<PathBuf> {
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data_home).join("opencode").join("auth.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json"))
}

pub fn read_credentials(path: &Path) -> std::result::Result<OpenCodeCredentials, ProviderError> {
    let bytes = fs::read(path).map_err(|_| ProviderError::MissingCredentials)?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        ProviderError::Unavailable("OpenCode auth file is not valid JSON".to_string())
    })?;
    find_key(&value).ok_or(ProviderError::MissingCredentials)
}

fn find_key(value: &Value) -> Option<OpenCodeCredentials> {
    let key = value
        .get(AUTH_PROVIDER_KEY)?
        .get("key")?
        .as_str()
        .filter(|key| !key.trim().is_empty())?;
    Some(OpenCodeCredentials {
        key: key.to_string(),
    })
}

pub fn parse_usage_response(
    value: &Value,
    fetched_at_unix: u64,
) -> std::result::Result<ProviderSnapshot, ProviderError> {
    let usage = value
        .get("usage")
        .ok_or_else(|| ProviderError::UnsupportedResponse("missing usage".to_string()))?;
    let mut windows = Vec::new();
    parse_window(usage.get("rolling"), WindowKind::FiveHour, &mut windows)?;
    parse_window(usage.get("weekly"), WindowKind::Weekly, &mut windows)?;
    parse_window(usage.get("monthly"), WindowKind::Monthly, &mut windows)?;
    if windows.is_empty() {
        return Err(ProviderError::UnsupportedResponse(
            "usage has no supported windows".to_string(),
        ));
    }
    Ok(ProviderSnapshot::new(
        Provider::OpenCodeGo,
        windows,
        fetched_at_unix,
    ))
}

fn parse_window(
    value: Option<&Value>,
    kind: WindowKind,
    windows: &mut Vec<UsageWindow>,
) -> std::result::Result<(), ProviderError> {
    // Only a reported-ok window is interpretable; anything else stays absent
    // rather than being guessed into a wrong number.
    if value
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        != Some("ok")
    {
        return Ok(());
    }
    let value = value.expect("status check guarantees a window object");
    let used = value
        .get("percent")
        .and_then(Value::as_f64)
        .ok_or_else(|| {
            ProviderError::UnsupportedResponse(format!("missing {} percent", kind.label()))
        })?;
    let reset = value
        .get("resetsAt")
        .and_then(Value::as_str)
        .and_then(ResetAt::parse_rfc3339);
    let window = UsageWindow::new(kind, used, reset)
        .map_err(|error| ProviderError::UnsupportedResponse(error.to_string()))?;
    windows.push(window);
    Ok(())
}

/// Map an HTTP failure to the same unavailable states other collectors use:
/// missing credentials for 401, an entitlement problem for 403.
pub fn error_from_status(status: u16, body: &Value) -> ProviderError {
    let detail = body
        .pointer("/error/message")
        .or_else(|| body.pointer("/error/type"))
        .and_then(Value::as_str);
    match status {
        401 => ProviderError::MissingCredentials,
        403 => ProviderError::Unavailable(
            detail
                .unwrap_or("OpenCode Go subscription required")
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
    fn reads_the_opencode_go_entry_from_top_level_provider_keys() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("auth.json");
        fs::write(
            &path,
            r#"{"openai":{"type":"api","key":"other"},"opencode-go":{"type":"api","key":"unit-test-key"},"xai":{}}"#,
        )
        .unwrap();
        assert_eq!(
            read_credentials(&path).unwrap(),
            OpenCodeCredentials {
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
        fs::write(&path, r#"{"opencode-go":{"type":"api","key":"   "}}"#).unwrap();
        assert_eq!(
            read_credentials(&path).unwrap_err().to_string(),
            "provider credentials are unavailable"
        );
    }

    #[test]
    fn auth_and_entitlement_errors_are_unavailable_never_zero() {
        assert!(matches!(
            error_from_status(401, &json!({"type":"error","error":{"type":"AuthError"}})),
            ProviderError::MissingCredentials
        ));
        let entitlement = error_from_status(
            403,
            &json!({
                "type":"error",
                "error":{"type":"EntitlementError","message":"OpenCode Go subscription required."}
            }),
        );
        assert_eq!(
            entitlement.to_string(),
            "provider quota is unavailable: OpenCode Go subscription required."
        );
    }
}
