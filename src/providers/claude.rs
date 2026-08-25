use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind};
use crate::providers::statusline::{enrich_cache_ttl, parse_context};
use crate::providers::ProviderError;
use serde_json::Value;

pub fn parse_statusline(
    value: &Value,
    fetched_at_unix: u64,
) -> std::result::Result<ProviderSnapshot, ProviderError> {
    let context = parse_context(
        value
            .get("context_window")
            .or_else(|| value.get("contextWindow")),
    )
    .unwrap_or(None);
    let Some(limits) = value.get("rate_limits") else {
        return Ok(
            ProviderSnapshot::new(Provider::Claude, vec![], fetched_at_unix).with_context(context),
        );
    };
    let mut windows = Vec::new();
    if let Some(window) = parse_window(limits.get("five_hour"), WindowKind::FiveHour)? {
        windows.push(window);
    }
    if let Some(window) = parse_window(limits.get("seven_day"), WindowKind::Weekly)? {
        windows.push(window);
    }
    if windows.is_empty() {
        return Ok(
            ProviderSnapshot::new(Provider::Claude, vec![], fetched_at_unix).with_context(context),
        );
    }
    Ok(ProviderSnapshot::new(Provider::Claude, windows, fetched_at_unix).with_context(context))
}

fn parse_window(
    value: Option<&Value>,
    kind: WindowKind,
) -> std::result::Result<Option<UsageWindow>, ProviderError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let used = value
        .get("used_percentage")
        .and_then(Value::as_f64)
        .ok_or_else(|| {
            ProviderError::UnsupportedResponse(format!("missing {} usage", kind.label()))
        })?;
    let reset = value.get("resets_at").and_then(|value| {
        value
            .as_u64()
            .map(ResetAt::from_unix_seconds)
            .or_else(|| value.as_str().and_then(ResetAt::parse))
    });
    UsageWindow::new(kind, used, reset)
        .map(Some)
        .map_err(|error| ProviderError::UnsupportedResponse(error.to_string()))
}

pub fn run_statusline(input: &[u8]) -> std::result::Result<ProviderSnapshot, ProviderError> {
    let value: Value = serde_json::from_slice(input).map_err(|_| {
        ProviderError::UnsupportedResponse("statusLine input is not JSON".to_string())
    })?;
    let mut snapshot = parse_statusline(&value, CacheStore::now_unix())?;
    enrich_cache_ttl(&mut snapshot, &value);
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_claude_five_hour_and_weekly_limits() {
        let value = json!({
            "rate_limits": {
                "five_hour": {"used_percentage": 58.0, "resets_at": 1786795200},
                "seven_day": {"used_percentage": 27.0, "resets_at": 1787400000}
            }
        });
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert_eq!(
            snapshot.window(WindowKind::FiveHour).unwrap().resets_at,
            Some(ResetAt::from_unix_seconds(1_786_795_200))
        );
    }

    #[test]
    fn parses_optional_context_window_usage() {
        let value = json!({
            "context_window": {
                "used_percentage": 23.5,
                "remaining_percentage": 76.5,
                "current_usage": {
                    "input_tokens": 100,
                    "cache_read_input_tokens": 800,
                    "cache_creation_input_tokens": 100
                }
            },
            "rate_limits": {
                "five_hour": {"used_percentage": 58.0}
            }
        });
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert_eq!(
            snapshot
                .context
                .as_ref()
                .map(|context| context.used_percent),
            Some(23.5)
        );
        let cache = snapshot.context.as_ref().unwrap().cache.as_ref().unwrap();
        assert_eq!(cache.read_tokens, 800);
        assert_eq!(cache.creation_tokens, 100);
        assert_eq!(cache.hit_percent, 80.0);
    }

    #[test]
    fn estimates_claude_cache_ttl_from_a_bounded_transcript_tail() {
        let transcript = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            transcript.path(),
            r#"{"type":"assistant","timestamp":"2026-08-22T10:00:00Z","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":80,"cache_creation_input_tokens":10,"cache_creation":{"ephemeral_1h_input_tokens":10,"ephemeral_5m_input_tokens":0}}}}"#,
        )
        .unwrap();
        let value = json!({
            "transcript_path": transcript.path(),
            "context_window": {
                "used_percentage": 23.5,
                "current_usage": {
                    "input_tokens": 10,
                    "cache_read_input_tokens": 80,
                    "cache_creation_input_tokens": 10
                }
            },
            "rate_limits": {"five_hour": {"used_percentage": 58.0}}
        });
        let snapshot = run_statusline(value.to_string().as_bytes()).unwrap();
        let cache = snapshot.context.unwrap().cache.unwrap();
        assert_eq!(cache.ttl_seconds, Some(60 * 60));
        assert_eq!(cache.last_activity_unix, Some(1_787_392_800));
    }

    #[test]
    fn parses_rfc3339_reset_emitted_by_claude_statusline() {
        let value = json!({
            "rate_limits": {
                "five_hour": {
                    "used_percentage": 57.0,
                    "resets_at": "2026-08-15T12:00:00Z"
                }
            }
        });
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert_eq!(
            snapshot.window(WindowKind::FiveHour).unwrap().resets_at,
            Some(ResetAt::from_unix_seconds(1_786_795_200))
        );
    }

    #[test]
    fn allows_a_missing_claude_window() {
        let value = json!({"rate_limits": {"five_hour": null}});
        assert!(parse_statusline(&value, 1).unwrap().windows.is_empty());
        let value = json!({
            "rate_limits": {"seven_day": {"used_percentage": 25.0}}
        });
        assert_eq!(parse_statusline(&value, 1).unwrap().windows.len(), 1);
    }

    #[test]
    fn accepts_a_payload_without_rate_limits_to_clear_a_stale_quota() {
        let value = json!({"context_window": {"used_percentage": 43.0}});
        let snapshot = parse_statusline(&value, 1).unwrap();
        assert!(snapshot.windows.is_empty());
        assert_eq!(
            snapshot
                .context
                .as_ref()
                .map(|context| context.used_percent),
            Some(43.0)
        );
    }

    #[test]
    fn rejects_non_json_statusline_input() {
        assert!(run_statusline(b"not-json").is_err());
    }
}
