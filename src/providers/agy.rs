use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind};
use crate::providers::statusline::parse_context;
use crate::providers::ProviderError;
use serde_json::Value;

const FIVE_HOUR_KEYS: [&str; 2] = ["gemini-5h", "3p-5h"];
const WEEKLY_KEYS: [&str; 2] = ["gemini-weekly", "3p-weekly"];

/// Parse the quota object emitted by Agy/Antigravity's statusLine JSON.
///
/// Agy reports separate Gemini and third-party (Claude/GPT) pools. The
/// sidebar has one Agy row, so each window is represented conservatively by
/// the lowest remaining percentage across the pools that are present.
pub fn parse_statusline(
    value: &Value,
    fetched_at_unix: u64,
) -> std::result::Result<ProviderSnapshot, ProviderError> {
    let quota = value
        .get("quota")
        .and_then(Value::as_object)
        .ok_or_else(|| ProviderError::UnsupportedResponse("missing quota".to_string()))?;
    let mut windows = Vec::new();
    for (kind, keys) in [
        (WindowKind::FiveHour, &FIVE_HOUR_KEYS[..]),
        (WindowKind::Weekly, &WEEKLY_KEYS[..]),
    ] {
        if let Some(window) = parse_window(quota, kind, keys, fetched_at_unix)? {
            windows.push(window);
        }
    }
    if windows.is_empty() {
        return Err(ProviderError::UnsupportedResponse(
            "quota has no supported windows".to_string(),
        ));
    }
    Ok(
        ProviderSnapshot::new(Provider::Agy, windows, fetched_at_unix).with_context(
            parse_context(
                value
                    .get("context_window")
                    .or_else(|| value.get("contextWindow")),
            )
            .unwrap_or(None),
        ),
    )
}

fn parse_window(
    quota: &serde_json::Map<String, Value>,
    kind: WindowKind,
    keys: &[&str],
    fetched_at_unix: u64,
) -> std::result::Result<Option<UsageWindow>, ProviderError> {
    let mut lowest_remaining: Option<f64> = None;
    let mut reset = None;
    for key in keys {
        let Some(bucket) = quota.get(*key) else {
            continue;
        };
        let Some(remaining) = parse_remaining(bucket) else {
            continue;
        };
        if lowest_remaining.is_none_or(|current| remaining < current) {
            lowest_remaining = Some(remaining);
            reset = parse_reset(bucket, fetched_at_unix);
        }
    }
    let Some(remaining) = lowest_remaining else {
        return Ok(None);
    };
    let used = (100.0 - remaining * 100.0).clamp(0.0, 100.0);
    UsageWindow::new(kind, used, reset)
        .map(Some)
        .map_err(|error| ProviderError::UnsupportedResponse(error.to_string()))
}

fn parse_reset(value: &Value, fetched_at_unix: u64) -> Option<ResetAt> {
    value
        .get("reset_time")
        .or_else(|| value.get("resetTime"))
        .and_then(Value::as_str)
        .and_then(ResetAt::parse_rfc3339)
        .or_else(|| {
            value
                .get("reset_in_seconds")
                .or_else(|| value.get("resetInSeconds"))
                .and_then(Value::as_u64)
                .map(|seconds| ResetAt::after(fetched_at_unix, seconds))
        })
}

fn parse_remaining(value: &Value) -> Option<f64> {
    let object = value.as_object()?;
    let raw = object
        .get("remaining_fraction")
        .or_else(|| object.get("remainingFraction"))
        .or_else(|| object.get("remaining_percent"))
        .or_else(|| object.get("remainingPercentage"))
        .and_then(Value::as_f64)?;
    if !raw.is_finite() {
        return None;
    }
    let fraction = if raw <= 1.0 { raw } else { raw / 100.0 };
    Some(fraction.clamp(0.0, 1.0))
}

pub fn run_statusline(input: &[u8]) -> std::result::Result<ProviderSnapshot, ProviderError> {
    let value: Value = serde_json::from_slice(input).map_err(|_| {
        ProviderError::UnsupportedResponse("statusLine input is not JSON".to_string())
    })?;
    parse_statusline(&value, CacheStore::now_unix())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_both_agy_windows_from_official_quota_keys() {
        let value = json!({
            "quota": {
                "gemini-5h": {"remaining_fraction": 0.9969, "reset_time": "2026-08-15T12:00:00Z"},
                "gemini-weekly": {"remaining_fraction": 0.8, "reset_time": "2026-08-22T12:00:00Z"},
                "3p-5h": {"remaining_fraction": 0.72, "reset_time": "2026-08-15T13:00:00Z"},
                "3p-weekly": {"remaining_fraction": 0.91}
            }
        });
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert_eq!(snapshot.provider, Provider::Agy);
        assert_eq!(
            snapshot.window(WindowKind::FiveHour).unwrap().resets_at,
            Some(ResetAt::from_unix_seconds(1_786_798_800))
        );
    }

    #[test]
    fn parses_optional_context_window_usage() {
        let value = json!({
            "context_window": {
                "used_percentage": 41.0,
                "current_usage": {
                    "input_tokens": 50,
                    "cache_read_input_tokens": 150,
                    "cache_creation_input_tokens": 0
                }
            },
            "quota": {
                "gemini-weekly": {"remaining_fraction": 0.8}
            }
        });
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert_eq!(
            snapshot
                .context
                .as_ref()
                .map(|context| context.used_percent),
            Some(41.0)
        );
        assert_eq!(
            snapshot
                .context
                .as_ref()
                .unwrap()
                .cache
                .as_ref()
                .unwrap()
                .hit_percent,
            75.0
        );
    }

    #[test]
    fn ignores_missing_pool_without_marking_the_window_unavailable() {
        let value = json!({
            "quota": {"gemini-weekly": {"remaining_fraction": 0.61}}
        });
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert!(snapshot.window(WindowKind::FiveHour).is_none());
    }

    #[test]
    fn derives_absolute_agy_reset_from_relative_seconds() {
        let value = json!({
            "quota": {"gemini-5h": {
                "remaining_fraction": 0.5,
                "reset_in_seconds": 900
            }}
        });
        let snapshot = parse_statusline(&value, 1_000).unwrap();
        assert_eq!(
            snapshot.window(WindowKind::FiveHour).unwrap().resets_at,
            Some(ResetAt::from_unix_seconds(1_900))
        );
    }

    #[test]
    fn rejects_payload_without_quota_windows() {
        assert!(parse_statusline(&json!({"quota": {}}), 1).is_err());
    }
}
