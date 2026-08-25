use crate::cache::CacheStore;
use crate::herdr::{current_agent_providers, list_agent_panes, publish_tokens, refresh_pane_topic};
use crate::model::Provider;
use crate::presentation::MetadataTokens;
use crate::providers::{codex, grok, opencode_go};
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ProviderOutcome {
    pub provider: Provider,
    pub available: bool,
    pub from_cache: bool,
    pub error: Option<String>,
}

pub fn run(providers: &[Provider], force: bool, json: bool) -> Result<()> {
    run_internal(providers, force, json, None)
}

fn run_internal(
    providers: &[Provider],
    force: bool,
    json: bool,
    topic_pane: Option<&str>,
) -> Result<()> {
    let cache = CacheStore::from_env()?;
    let outcomes = cache.with_lock(|| refresh_locked(&cache, providers, force))?;
    publish(&cache, providers, topic_pane)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&outcomes)?);
    }
    Ok(())
}

pub fn event() -> Result<()> {
    let event = event_json();
    // Unknown agent kinds keep the historical fallback of refreshing every
    // provider; multi-agent kinds (opencode) refresh all of their sources.
    let providers = event
        .as_ref()
        .and_then(find_agent)
        .and_then(Provider::providers_for_agent)
        .unwrap_or_else(|| Provider::ALL.to_vec());
    let topic_pane = event.as_ref().and_then(find_pane_id);
    run_internal(&providers, false, false, topic_pane)
}

pub fn focus() -> Result<()> {
    let providers = current_agent_providers()?;
    if providers.is_empty() {
        return Ok(());
    }
    run(&providers, false, false)
}

/// Refresh one provider under the state-dir lock with the standard 60s
/// debounce, skipping pane publication entirely. The status poll uses this so
/// the Go collector gets refreshed without ever touching a pane.
pub fn debounced_refresh(provider: Provider) -> Result<()> {
    let cache = CacheStore::from_env()?;
    cache.with_lock(|| refresh_locked(&cache, &[provider], false))?;
    Ok(())
}

fn refresh_locked(
    cache: &CacheStore,
    providers: &[Provider],
    force: bool,
) -> Result<Vec<ProviderOutcome>> {
    let now = CacheStore::now_unix();
    let mut outcomes = Vec::new();
    for provider in providers {
        if !force && cache.should_debounce(*provider, now, 60)? {
            outcomes.push(ProviderOutcome {
                provider: *provider,
                available: cache.load(*provider)?.is_some(),
                from_cache: true,
                error: None,
            });
            continue;
        }
        let fetched = match provider {
            Provider::Codex => codex::fetch(),
            Provider::Grok => grok::fetch(),
            Provider::OpenCodeGo => opencode_go::fetch(),
            Provider::Claude => match cache.load(Provider::Claude)? {
                Some(snapshot) => Ok(snapshot),
                None => Err(anyhow::anyhow!(
                    "Claude usage is collected by the Claude statusLine hook"
                )),
            },
            Provider::Agy => match cache.load(Provider::Agy)? {
                Some(snapshot) => Ok(snapshot),
                None => Err(anyhow::anyhow!(
                    "Agy usage is collected by the Agy statusLine hook"
                )),
            },
        };
        cache.mark_refresh(*provider, now)?;
        match fetched {
            Ok(snapshot) => {
                if !matches!(provider, Provider::Claude | Provider::Agy) {
                    cache.save(&snapshot)?;
                }
                outcomes.push(ProviderOutcome {
                    provider: *provider,
                    available: true,
                    from_cache: false,
                    error: None,
                });
            }
            Err(error) => outcomes.push(ProviderOutcome {
                provider: *provider,
                available: cache.load(*provider)?.is_some(),
                from_cache: true,
                error: Some(error.to_string()),
            }),
        }
    }
    Ok(outcomes)
}

fn publish(cache: &CacheStore, providers: &[Provider], topic_pane: Option<&str>) -> Result<()> {
    let mut panes = list_agent_panes().unwrap_or_default();
    if let Some(pane) = topic_pane.and_then(|pane_id| {
        panes.iter_mut().find(|pane| {
            pane.pane_id == pane_id
                && pane
                    .providers
                    .iter()
                    .any(|provider| providers.contains(provider))
        })
    }) {
        refresh_pane_topic(pane);
    }
    let mut tokens = Vec::new();
    let now = CacheStore::now_unix();
    for provider in providers {
        let snapshot = cache.load(*provider)?;
        if let Some(values) = tokens_for_provider(snapshot.as_ref(), now) {
            tokens.push((*provider, values));
        }
    }
    publish_tokens(&panes, &tokens, CacheStore::now_millis())
}

fn event_json() -> Option<Value> {
    let input = std::env::var("HERDR_PLUGIN_EVENT_JSON").ok()?;
    serde_json::from_str(&input).ok()
}

// Event payloads are nested and their shape differs per event, so look the
// field up anywhere in the tree rather than at a fixed path.
fn find_field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a str> {
    match value {
        Value::Object(map) => names
            .iter()
            .find_map(|name| map.get(*name).and_then(Value::as_str))
            .or_else(|| map.values().find_map(|child| find_field(child, names))),
        Value::Array(values) => values.iter().find_map(|child| find_field(child, names)),
        _ => None,
    }
}

fn find_agent(value: &Value) -> Option<&str> {
    find_field(value, &["agent"])
}

fn find_pane_id(value: &Value) -> Option<&str> {
    find_field(value, &["pane_id", "paneId"])
}

fn tokens_for_provider(
    snapshot: Option<&crate::model::ProviderSnapshot>,
    now_unix: u64,
) -> Option<MetadataTokens> {
    snapshot.map(|snapshot| MetadataTokens::from_snapshot(snapshot, now_unix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProviderSnapshot, UsageWindow, WindowKind};
    use tempfile::tempdir;

    #[test]
    fn successful_snapshot_is_kept_when_provider_refresh_fails() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let snapshot = ProviderSnapshot::new(
            Provider::Grok,
            vec![UsageWindow::new(WindowKind::Weekly, 42.5, None).unwrap()],
            1,
        );
        cache.save(&snapshot).unwrap();
        assert_eq!(cache.load(Provider::Grok).unwrap(), Some(snapshot));
    }

    #[test]
    fn missing_snapshot_does_not_overwrite_sidebar_with_unavailable() {
        let values = tokens_for_provider(None, 1);
        assert!(values.is_none());
    }

    // Reading a pane repaints it, which visibly scrolls the agent's terminal.
    // An event must name exactly one pane to read, so the other panes of the
    // same provider are left alone.
    #[test]
    fn event_payload_names_the_single_pane_whose_topic_may_be_read() {
        let value: Value = serde_json::from_str(
            r#"{"event":"pane_agent_status_changed",
                "data":{"pane_id":"w1:p2","agent":"grok","status":"working"}}"#,
        )
        .unwrap();
        assert_eq!(find_pane_id(&value), Some("w1:p2"));
        assert_eq!(find_agent(&value), Some("grok"));
    }

    #[test]
    fn an_event_without_a_pane_reads_no_pane_at_all() {
        let value: Value =
            serde_json::from_str(r#"{"event":"x","data":{"agent":"claude"}}"#).unwrap();
        assert_eq!(find_pane_id(&value), None);
    }
}
