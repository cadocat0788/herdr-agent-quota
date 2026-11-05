use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind};
use crate::process::run_shell_with_deadline;
use crate::providers::ProviderError;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";
// OpenCode 2 keeps credentials in its own store and exposes them through the
// CLI. Both entries are needed: the Console OAuth session authorizes the usage
// endpoint, while the Go subscription key is the legacy fallback.
const CONSOLE_INTEGRATION_KEY: &str = "opencode";
const AUTH_PROVIDER_KEY: &str = "opencode-go";
const AUTH_EXPORT_COMMAND: &str = "opencode auth export";
const AUTH_EXPORT_BUDGET: Duration = Duration::from_secs(5);
// Refresh OAuth a minute early so a token cannot expire mid-request.
const OAUTH_EXPIRY_SKEW_MS: u64 = 60_000;
const EXPIRED_SESSION_HINT: &str =
    "OpenCode Console session expired; run `opencode auth login opencode` to reconnect";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeCredentials {
    pub key: String,
}

/// The credential-bearing subset of `opencode auth export`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExportedCredentials {
    pub console_access: Option<String>,
    pub console_expires_ms: Option<u64>,
    pub api_key: Option<String>,
}

pub fn fetch() -> Result<ProviderSnapshot> {
    let credentials = select_credentials(
        read_export().as_deref(),
        auth_path().ok().and_then(|path| read_credentials(&path).ok()),
        now_unix_millis(),
    )
    .map_err(anyhow::Error::new)?;
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

/// Resolve the credential to send: a live Console OAuth session first (it
/// authorizes the usage endpoint and cannot expire silently), then the v2 Go
/// subscription key, then the legacy `auth.json` key.
pub fn select_credentials(
    export: Option<&str>,
    legacy: Option<OpenCodeCredentials>,
    now_ms: u64,
) -> std::result::Result<OpenCodeCredentials, ProviderError> {
    let exported = export.map(parse_export).unwrap_or_default();
    if let Some(access) = exported.console_access.clone() {
        let usable = exported
            .console_expires_ms
            .is_some_and(|expires| now_ms.saturating_add(OAUTH_EXPIRY_SKEW_MS) < expires);
        if usable {
            return Ok(OpenCodeCredentials { key: access });
        }
    }
    if let Some(key) = exported.api_key.clone() {
        return Ok(OpenCodeCredentials { key });
    }
    if let Some(legacy) = legacy {
        return Ok(legacy);
    }
    if exported.console_access.is_some() {
        return Err(ProviderError::Unavailable(EXPIRED_SESSION_HINT.to_string()));
    }
    Err(ProviderError::MissingCredentials)
}

/// Read the v2 credential store. A missing, slow, or failed CLI is not an
/// error: the caller falls through to the next credential source.
fn read_export() -> Option<String> {
    let output = run_shell_with_deadline(AUTH_EXPORT_COMMAND, b"", AUTH_EXPORT_BUDGET).ok()?;
    if output.timed_out || output.exit_code != Some(0) {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

pub fn parse_export(export: &str) -> ExportedCredentials {
    let Some(items) = serde_json::from_str::<Value>(export)
        .ok()
        .and_then(|value| value.as_array().cloned())
    else {
        return ExportedCredentials::default();
    };
    let mut parsed = ExportedCredentials::default();
    for item in items {
        let value = item.get("value");
        match item.get("integrationID").and_then(Value::as_str) {
            Some(CONSOLE_INTEGRATION_KEY) if parsed.console_access.is_none() => {
                parsed.console_access = value
                    .and_then(|value| value.get("access"))
                    .and_then(Value::as_str)
                    .filter(|token| !token.trim().is_empty())
                    .map(str::to_string);
                parsed.console_expires_ms = value
                    .and_then(|value| value.get("expires"))
                    .and_then(timestamp_millis);
            }
            Some(AUTH_PROVIDER_KEY) if parsed.api_key.is_none() => {
                parsed.api_key = value
                    .and_then(|value| value.get("key"))
                    .and_then(Value::as_str)
                    .filter(|key| !key.trim().is_empty())
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    parsed
}

fn timestamp_millis(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_f64()
            .filter(|millis| *millis > 0.0)
            .map(|millis| millis as u64)
    })
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
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

    const CONSOLE_TOKEN: &str = "console-access-token-fixture";
    const GO_KEY: &str = "go-api-key-fixture";

    fn export_item(integration: &str, value: serde_json::Value) -> serde_json::Value {
        json!({
            "id": format!("cred_{integration}"),
            "integrationID": integration,
            "label": "fixture",
            "value": value,
        })
    }

    fn console_oauth(expires: u64) -> serde_json::Value {
        export_item(
            "opencode",
            json!({
                "type": "oauth",
                "access": CONSOLE_TOKEN,
                "refresh": "console-refresh-fixture",
                "expires": expires,
            }),
        )
    }

    fn subscription_key() -> serde_json::Value {
        export_item(
            "opencode-go",
            json!({ "type": "key", "key": GO_KEY }),
        )
    }

    fn exported(items: Vec<serde_json::Value>) -> String {
        serde_json::Value::Array(items).to_string()
    }

    #[test]
    fn console_oauth_is_preferred_over_the_subscription_key() {
        let raw = exported(vec![console_oauth(2_000_000), subscription_key()]);
        assert_eq!(
            select_credentials(Some(&raw), None, 1_000_000).unwrap(),
            OpenCodeCredentials {
                key: CONSOLE_TOKEN.to_string()
            }
        );
    }

    #[test]
    fn expired_console_oauth_falls_back_to_the_subscription_key() {
        let raw = exported(vec![console_oauth(1_000_000), subscription_key()]);
        assert_eq!(
            select_credentials(Some(&raw), None, 1_000_000).unwrap(),
            OpenCodeCredentials {
                key: GO_KEY.to_string()
            }
        );
    }

    #[test]
    fn expired_console_oauth_without_a_key_is_actionable_and_never_leaks_the_token() {
        let raw = exported(vec![console_oauth(1_000_000)]);
        let message = select_credentials(Some(&raw), None, 1_000_000)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("opencode auth login opencode"),
            "{message}"
        );
        assert!(!message.contains(CONSOLE_TOKEN), "{message}");
    }

    #[test]
    fn legacy_auth_file_key_is_the_last_credential_source() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("auth.json");
        fs::write(
            &path,
            format!(r#"{{"opencode-go":{{"type":"api","key":"{GO_KEY}"}}}}"#),
        )
        .unwrap();
        let legacy = read_credentials(&path).unwrap();
        assert_eq!(
            select_credentials(Some("[]"), Some(legacy), 1_000_000).unwrap(),
            OpenCodeCredentials {
                key: GO_KEY.to_string()
            }
        );
    }

    #[test]
    fn missing_every_credential_source_is_missing_credentials() {
        assert!(matches!(
            select_credentials(None, None, 1_000_000),
            Err(ProviderError::MissingCredentials)
        ));
        assert!(matches!(
            select_credentials(Some("[]"), None, 1_000_000),
            Err(ProviderError::MissingCredentials)
        ));
    }

    #[test]
    fn malformed_export_falls_through_to_the_legacy_file() {
        let legacy = OpenCodeCredentials {
            key: GO_KEY.to_string(),
        };
        for raw in [
            "",
            "not json",
            "{}",
            r#"[{"integrationID":"opencode"}]"#,
            r#"[{"integrationID":"opencode","value":{"access":"  ","expires":2000000}}]"#,
        ] {
            assert_eq!(
                select_credentials(Some(raw), Some(legacy.clone()), 1_000_000).unwrap(),
                legacy,
                "raw={raw}"
            );
        }
    }

    #[test]
    fn parse_export_reads_both_integrations_and_ignores_unknown_ones() {
        let raw = exported(vec![
            export_item(
                "openai",
                json!({ "type": "oauth", "access": "other-token", "expires": 9 }),
            ),
            console_oauth(2_000_000),
            subscription_key(),
        ]);
        let parsed = parse_export(&raw);
        assert_eq!(parsed.console_access.as_deref(), Some(CONSOLE_TOKEN));
        assert_eq!(parsed.console_expires_ms, Some(2_000_000));
        assert_eq!(parsed.api_key.as_deref(), Some(GO_KEY));
    }

    #[test]
    fn no_credential_selection_error_contains_token_material() {
        let cases = [
            exported(vec![console_oauth(1_000_000)]),
            exported(vec![subscription_key()]),
        ];
        for raw in cases {
            if let Err(error) = select_credentials(Some(&raw), None, 1_000_000) {
                let message = error.to_string();
                assert!(!message.contains(CONSOLE_TOKEN), "{message}");
                assert!(!message.contains(GO_KEY), "{message}");
            }
        }
    }
}
