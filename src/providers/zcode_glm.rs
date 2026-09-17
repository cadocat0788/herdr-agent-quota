use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind};
use crate::providers::ProviderError;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const QUOTA_URL: &str = "https://api.z.ai/api/monitor/usage/quota/limit";

// The Zcode GLM coding-plan key lives in Pi's auth store under this top-level
// entry; only this entry holds the GLM subscription credential.
const AUTH_ENTRY_KEY: &str = "zai-coding-cn";

// data.limits[].unit encodes the reset cadence: 3 = ~5 hours, 6 = weekly.
const UNIT_FIVE_HOUR: i64 = 3;
const UNIT_WEEKLY: i64 = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZcodeGlmCredentials {
    pub key: String,
}

pub fn fetch() -> Result<ProviderSnapshot> {
    let path = auth_path().context("resolve Zcode GLM auth path")?;
    let credentials = read_credentials(&path).map_err(anyhow::Error::from)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(10))
        .timeout_write(Duration::from_secs(10))
        .build();
    let response = agent
        .get(QUOTA_URL)
        .set("Authorization", &format!("Bearer {}", credentials.key))
        .set("Accept", "application/json")
        .call();
    match response {
        Ok(response) => {
            let value: Value = response
                .into_json()
                .context("decode Zcode GLM quota response")?;
            parse_quota_response(&value, CacheStore::now_unix()).map_err(anyhow::Error::from)
        }
        Err(ureq::Error::Status(code, _)) => Err(anyhow::Error::new(ProviderError::Request(
            format!("HTTP {code}"),
        ))),
        Err(error) => Err(anyhow::Error::new(ProviderError::Request(
            http_error_status(&error),
        ))),
    }
}

/// `ZCODE_GLM_AUTH_FILE` override, else Pi's auth store `~/.pi/agent/auth.json`.
pub fn auth_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ZCODE_GLM_AUTH_FILE") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".pi")
        .join("agent")
        .join("auth.json"))
}

pub fn read_credentials(path: &Path) -> std::result::Result<ZcodeGlmCredentials, ProviderError> {
    let bytes = fs::read(path).map_err(|_| ProviderError::MissingCredentials)?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ProviderError::Unavailable("Zcode GLM auth file is not valid JSON".into()))?;
    find_key(&value).ok_or(ProviderError::MissingCredentials)
}

fn find_key(value: &Value) -> Option<ZcodeGlmCredentials> {
    let key = value
        .get(AUTH_ENTRY_KEY)?
        .get("key")?
        .as_str()
        .filter(|key| !key.trim().is_empty())?;
    Some(ZcodeGlmCredentials {
        key: key.to_string(),
    })
}

pub fn parse_quota_response(
    value: &Value,
    fetched_at_unix: u64,
) -> std::result::Result<ProviderSnapshot, ProviderError> {
    // The endpoint wraps payloads in {code, msg, data, success}; a non-success
    // envelope is a provider-side refusal, not a parse problem.
    if value.get("success").and_then(Value::as_bool) != Some(true) {
        let message = value
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("quota request refused");
        return Err(ProviderError::Unavailable(message.to_string()));
    }
    let data = value
        .get("data")
        .ok_or_else(|| ProviderError::UnsupportedResponse("missing data".to_string()))?;
    let limits = data
        .get("limits")
        .and_then(Value::as_array)
        .ok_or_else(|| ProviderError::UnsupportedResponse("missing limits".to_string()))?;
    let mut windows = Vec::new();
    for limit in limits {
        parse_limit(limit, &mut windows)?;
    }
    if windows.is_empty() {
        return Err(ProviderError::UnsupportedResponse(
            "limits have no supported windows".to_string(),
        ));
    }
    Ok(ProviderSnapshot::new(
        Provider::ZcodeGlm,
        windows,
        fetched_at_unix,
    ))
}

fn parse_limit(
    limit: &Value,
    windows: &mut Vec<UsageWindow>,
) -> std::result::Result<(), ProviderError> {
    let kind = match limit.get("unit").and_then(Value::as_i64) {
        Some(unit) if unit == UNIT_FIVE_HOUR => WindowKind::FiveHour,
        Some(unit) if unit == UNIT_WEEKLY => WindowKind::Weekly,
        // Unknown cadences stay absent rather than guessed into a wrong slot.
        _ => return Ok(()),
    };
    // `percentage` is percent USED, matching UsageWindow::new's contract
    // (it derives remaining = 100 - used internally).
    let used = limit
        .get("percentage")
        .and_then(Value::as_f64)
        .ok_or_else(|| ProviderError::UnsupportedResponse("limit missing percentage".to_string()))?;
    // Out-of-range endpoint values clamp before window validation rejects them.
    let used = used.clamp(0.0, 100.0);
    let resets_at = limit
        .get("nextResetTime")
        .and_then(Value::as_i64)
        .filter(|millis| *millis > 0)
        .map(|millis| ResetAt::from_unix_seconds((millis / 1000) as u64));
    windows.push(UsageWindow::new(kind, used, resets_at).map_err(|_| {
        ProviderError::UnsupportedResponse("invalid usage window values".to_string())
    })?);
    Ok(())
}

fn http_error_status(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        ureq::Error::Transport(_) => "transport error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn envelope() -> Value {
        json!({
            "code": 200,
            "msg": "Operation successful",
            "success": true,
            "data": {
                "level": "lite",
                "limits": [
                    {"type": "CREDIT_LIMIT", "unit": 3, "number": 5, "usage": 2000,
                     "currentValue": 339, "remaining": 1660, "percentage": 16,
                     "nextResetTime": 1789618196040i64},
                    {"type": "CREDIT_LIMIT", "unit": 6, "number": 1, "usage": 10000,
                     "currentValue": 339, "remaining": 9660, "percentage": 3,
                     "nextResetTime": 1790204448984i64}
                ]
            }
        })
    }

    #[test]
    fn parses_five_hour_and_weekly_windows_as_remaining() {
        let snapshot = parse_quota_response(&envelope(), 1000).unwrap();
        assert_eq!(snapshot.provider, Provider::ZcodeGlm);
        let five_hour = snapshot.window(WindowKind::FiveHour).unwrap();
        let weekly = snapshot.window(WindowKind::Weekly).unwrap();
        assert!((five_hour.remaining_percent - 84.0).abs() < 1e-9);
        assert!((weekly.remaining_percent - 97.0).abs() < 1e-9);
        assert_eq!(
            five_hour.resets_at,
            Some(ResetAt::from_unix_seconds(1_789_618_196))
        );
        assert_eq!(
            weekly.resets_at,
            Some(ResetAt::from_unix_seconds(1_790_204_448))
        );
    }

    #[test]
    fn unknown_units_are_skipped_without_failing_the_snapshot() {
        let mut value = envelope();
        value["data"]["limits"][0]["unit"] = json!(9);
        let snapshot = parse_quota_response(&value, 1000).unwrap();
        assert!(snapshot.window(WindowKind::FiveHour).is_none());
        assert!(snapshot.window(WindowKind::Weekly).is_some());
    }

    #[test]
    fn missing_percentage_is_an_unsupported_response() {
        let mut value = envelope();
        value["data"]["limits"][0].as_object_mut().unwrap().remove("percentage");
        assert!(parse_quota_response(&value, 1000).is_err());
    }

    #[test]
    fn success_false_surfaces_the_provider_message() {
        let mut value = envelope();
        value["success"] = json!(false);
        value["msg"] = json!("forbidden");
        let error = parse_quota_response(&value, 1000).unwrap_err();
        assert_eq!(
            error,
            ProviderError::Unavailable("forbidden".to_string())
        );
    }

    #[test]
    fn clamps_percentages_out_of_range() {
        let mut value = envelope();
        value["data"]["limits"][0]["percentage"] = json!(120);
        value["data"]["limits"][1]["percentage"] = json!(-5);
        let snapshot = parse_quota_response(&value, 1000).unwrap();
        assert_eq!(snapshot.window(WindowKind::FiveHour).unwrap().remaining_percent, 0.0);
        assert_eq!(snapshot.window(WindowKind::Weekly).unwrap().remaining_percent, 100.0);
    }

    #[test]
    fn reads_the_pi_auth_entry() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("auth.json");
        std::fs::write(
            &path,
            json!({"zai-coding-cn": {"type": "api_key", "key": "abc123"}}).to_string(),
        )
        .unwrap();
        let credentials = read_credentials(&path).unwrap();
        assert_eq!(credentials.key, "abc123");
    }

    #[test]
    fn missing_entry_is_missing_credentials() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("auth.json");
        std::fs::write(&path, json!({"other": {"key": "x"}}).to_string()).unwrap();
        assert_eq!(
            read_credentials(&path).unwrap_err(),
            ProviderError::MissingCredentials
        );
    }
}
