use crate::model::Provider;
use crate::presentation::MetadataTokens;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

const METADATA_TTL_MS: &str = "86400000";
const MAX_METADATA_TOKENS: usize = 16;
const METADATA_TOKEN_NAMES: [&str; 16] = [
    "quota_state",
    "quota_provider",
    "quota_summary",
    "quota_context",
    "quota_cache",
    "quota_cache_ttl",
    "quota_5h",
    "quota_5h_normal",
    "quota_5h_warning",
    "quota_5h_danger",
    "quota_week",
    "quota_week_normal",
    "quota_week_warning",
    "quota_week_danger",
    "quota_topic",
    "quota_error",
];
const OBSOLETE_METADATA_TOKEN_NAMES: [&str; 2] = ["quota_icon", "quota_status"];
const LEGACY_METADATA_TOKEN_NAMES: [&str; 2] = ["quota_badge", "quota_session"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPane {
    pub pane_id: String,
    pub providers: Vec<Provider>,
    pub session_id: Option<String>,
    pub session_summary: String,
    pub topic: String,
    pub tokens: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentState {
    pub panes: Vec<AgentPane>,
    pub working_providers: Vec<Provider>,
}

pub fn list_agent_panes() -> Result<Vec<AgentPane>> {
    Ok(list_agent_state()?.panes)
}

/// Read Herdr's agent inventory once and derive both panes and working
/// providers from that same response. The active-turn watcher uses this
/// combined view so one poll does not fan out into one `agent list` call per
/// provider.
pub fn list_agent_state() -> Result<AgentState> {
    let value = list_agent_value()?;
    let mut panes = Vec::new();
    collect_agent_panes(&value, &mut panes);
    panes.sort_by(|left, right| left.pane_id.cmp(&right.pane_id));
    panes.dedup_by(|left, right| left.pane_id == right.pane_id);
    Ok(AgentState {
        panes,
        working_providers: working_providers_from(&value),
    })
}

/// Return whether at least one pane for a provider is currently working.
///
/// This provider-specific helper only asks Herdr for agent metadata; it never
/// reads terminal output. The global watcher uses [`list_agent_state`] so all
/// providers share one inventory call per poll.
pub fn provider_has_working_agent(provider: Provider) -> Result<bool> {
    Ok(list_agent_state()?.working_providers.contains(&provider))
}

fn list_agent_value() -> Result<Value> {
    let executable = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let output = Command::new(&executable)
        .args(["agent", "list"])
        .output()
        .context("list Herdr agents")?;
    if !output.status.success() {
        anyhow::bail!("Herdr agent list failed with {}", output.status);
    }
    serde_json::from_slice(&output.stdout).context("parse Herdr agent list")
}

pub fn current_agent_providers() -> Result<Vec<Provider>> {
    if let Some(agent) = std::env::var_os("HERDR_FOCUSED_PANE_AGENT") {
        if let Some(providers) = Provider::providers_for_agent(&agent.to_string_lossy()) {
            return Ok(providers);
        }
    }
    let executable = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let output = Command::new(executable)
        .args(["pane", "current"])
        .output()
        .context("read focused Herdr pane")?;
    if !output.status.success() {
        anyhow::bail!("Herdr pane current failed with {}", output.status);
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).context("parse focused Herdr pane")?;
    Ok(value
        .pointer("/result/pane/agent")
        .and_then(Value::as_str)
        .and_then(Provider::providers_for_agent)
        .unwrap_or_default())
}

// Reading a pane makes Herdr repaint it, which visibly scrolls the agent's
// terminal. Only the pane that fired the event is worth that cost; every other
// pane keeps the topic it last published.
pub fn refresh_pane_topic(pane: &mut AgentPane) {
    // Multi-agent cards (opencode) interleave several agents' prompt styles,
    // so marker matching cannot tell which line is the user's prompt yet.
    // Reading would repaint the pane and still find nothing; marker tuning is
    // deferred until it can be verified against a live pane.
    let [provider] = pane.providers.as_slice() else {
        return;
    };
    let executable = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    if let Some(topic) = read_pane_topic(&executable, pane, provider) {
        pane.topic = topic;
    }
}

fn collect_agent_panes(value: &Value, panes: &mut Vec<AgentPane>) {
    match value {
        Value::Object(map) => {
            let pane_id = map
                .get("pane_id")
                .or_else(|| map.get("paneId"))
                .and_then(Value::as_str);
            let kind = map
                .get("agent")
                .and_then(Value::as_str)
                .or_else(|| map.get("kind").and_then(Value::as_str))
                .or_else(|| {
                    map.get("agent_session")
                        .and_then(Value::as_object)
                        .and_then(|session| session.get("agent"))
                        .and_then(Value::as_str)
                });
            if let (Some(pane_id), Some(kind)) = (pane_id, kind) {
                if let Some(providers) = Provider::providers_for_agent(kind) {
                    let tokens: BTreeMap<String, String> = map
                        .get("tokens")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|tokens| tokens.iter())
                        .filter_map(|(name, value)| {
                            value
                                .as_str()
                                .map(|value| (name.clone(), value.to_string()))
                        })
                        .collect();
                    let topic = tokens.get("quota_topic").cloned().unwrap_or_default();
                    let session_summary = tokens.get("quota_session").cloned().unwrap_or_default();
                    let session_id = map
                        .get("agent_session")
                        .and_then(Value::as_object)
                        .and_then(|session| session.get("value"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    panes.push(AgentPane {
                        pane_id: pane_id.to_string(),
                        providers,
                        session_id,
                        session_summary,
                        // Preserve the last published topic during quota-only
                        // refreshes. Agent events refresh it from pane output.
                        topic,
                        tokens,
                    });
                }
            }
            for child in map.values() {
                collect_agent_panes(child, panes);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_agent_panes(child, panes);
            }
        }
        _ => {}
    }
}

fn working_providers_from(value: &Value) -> Vec<Provider> {
    let mut providers = Vec::new();
    collect_working_providers(value, &mut providers);
    providers.sort_by_key(|provider| {
        Provider::ALL
            .iter()
            .position(|candidate| candidate == provider)
    });
    providers.dedup();
    providers
}

fn collect_working_providers(value: &Value, providers: &mut Vec<Provider>) {
    match value {
        Value::Object(map) => {
            let kind = map
                .get("agent")
                .and_then(Value::as_str)
                .or_else(|| map.get("kind").and_then(Value::as_str))
                .or_else(|| {
                    map.get("agent_session")
                        .and_then(Value::as_object)
                        .and_then(|session| session.get("agent"))
                        .and_then(Value::as_str)
                });
            let status = map
                .get("agent_status")
                .or_else(|| map.get("agentStatus"))
                .or_else(|| map.get("status"))
                .or_else(|| map.get("state"))
                .and_then(Value::as_str);
            if let (Some(kind), Some(status)) = (kind, status) {
                if status.eq_ignore_ascii_case("working") {
                    if let Ok(provider) = kind.parse::<Provider>() {
                        providers.push(provider);
                    }
                }
            }
            for child in map.values() {
                collect_working_providers(child, providers);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_working_providers(child, providers);
            }
        }
        _ => {}
    }
}

pub fn publish_tokens(
    panes: &[AgentPane],
    tokens: &[(Provider, MetadataTokens)],
    sequence: u64,
) -> Result<()> {
    let executable = std::env::var_os("HERDR_BIN_PATH").unwrap_or_else(|| "herdr".into());
    let mut reported = 0usize;
    let mut failed = Vec::new();
    for pane in panes {
        // A pane is served when any of its identities has usable data.
        // Single-provider panes resolve exactly as before; multi-agent cards
        // (opencode) combine their providers' windows onto one card.
        let supported: Vec<&(Provider, MetadataTokens)> = pane
            .providers
            .iter()
            .filter_map(|provider| tokens.iter().find(|(candidate, _)| candidate == provider))
            .collect();
        if supported.is_empty() {
            continue;
        }
        let topic = display_topic(pane);
        let desired = if pane.providers.len() > 1 {
            desired_multi_provider_tokens(&pane.providers, tokens, &topic)
        } else {
            let (_, values) = supported[0];
            desired_tokens(values, &topic)
        };
        if metadata_matches(&pane.tokens, &desired) {
            continue;
        }
        // Herdr versions that repaint metadata can snap a terminal viewport
        // back to the bottom. Never mutate pane metadata while the user is
        // reading scrollback; the next refresh after they return catches up.
        if pane_is_scrolled(&executable, &pane.pane_id) {
            continue;
        }
        reported += 1;
        let mut command = Command::new(&executable);
        command
            .args([
                "pane",
                "report-metadata",
                &pane.pane_id,
                "--source",
                "herdr-agent-quota",
            ])
            .args(["--seq", &sequence.to_string()])
            .args(["--ttl-ms", METADATA_TTL_MS]);
        for name in metadata_report_names(pane, &desired) {
            if let Some(value) = desired.get(name) {
                command.args(["--token", &format!("{name}={value}")]);
            } else {
                command.args(["--clear-token", name]);
            }
        }
        let output = command.output().context("report quota metadata to Herdr")?;
        if !output.status.success() {
            failed.push(pane.pane_id.clone());
        }
    }
    // A pane can exit between `agent list` and this report, and the exit event
    // itself triggers a publish. One stale pane id must not stop the panes
    // that are still alive from being updated.
    if reported > 0 && failed.len() == reported {
        anyhow::bail!(
            "Herdr metadata report failed for every pane: {}",
            failed.join(", ")
        );
    }
    Ok(())
}

fn pane_is_scrolled(executable: &std::ffi::OsStr, pane_id: &str) -> bool {
    let Ok(output) = Command::new(executable)
        .args(["pane", "get", pane_id])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    serde_json::from_slice::<Value>(&output.stdout)
        .ok()
        .and_then(|value| {
            value
                .pointer("/result/pane/scroll/offset_from_bottom")
                .and_then(Value::as_u64)
        })
        .is_some_and(|offset| offset > 0)
}

fn desired_tokens(values: &MetadataTokens, topic: &str) -> BTreeMap<String, String> {
    let mut tokens = BTreeMap::from([
        ("quota_state".to_string(), values.quota_state.clone()),
        ("quota_provider".to_string(), values.quota_provider.clone()),
        ("quota_summary".to_string(), values.quota_summary.clone()),
    ]);
    insert_optional_token(&mut tokens, "quota_context", &values.quota_context);
    insert_optional_token(&mut tokens, "quota_cache", &values.quota_cache);
    insert_optional_token(&mut tokens, "quota_cache_ttl", &values.quota_cache_ttl);
    insert_optional_token(&mut tokens, "quota_5h", &values.quota_5h);
    insert_severity_token(
        &mut tokens,
        "quota_5h",
        &values.quota_5h,
        values.quota_5h_severity,
    );
    insert_optional_token(&mut tokens, "quota_week", &values.quota_week);
    insert_severity_token(
        &mut tokens,
        "quota_week",
        &values.quota_week,
        values.quota_week_severity,
    );
    insert_optional_token(&mut tokens, "quota_topic", topic);
    if let Some(error) = &values.quota_error {
        tokens.insert("quota_error".to_string(), error.clone());
    }
    tokens
}

// A multi-agent card (opencode) shows two independent subscriptions on one
// pane. Their weekly windows are placed into Herdr's two window groups
// without introducing token names: Codex fills $quota_week_* and Grok fills
// $quota_5h_*, which stays absent for every single-provider pane today. Zero
// schema changes, each group keeps its own severity coloring, and Herdr
// elides absent groups when only one subscription has data. Each value is
// prefixed with its provider's lowercase kind name so the two rows stay
// distinguishable; single-provider panes never see these slots.
fn desired_multi_provider_tokens(
    providers: &[Provider],
    tokens: &[(Provider, MetadataTokens)],
    topic: &str,
) -> BTreeMap<String, String> {
    let mut desired = BTreeMap::new();

    // Shared identity slots follow the worst severity; equal severities keep
    // the earlier provider, and providers_for_agent lists Codex before Grok.
    let mut identity: Option<(u8, &MetadataTokens)> = None;
    for provider in providers {
        let Some((_, values)) = tokens.iter().find(|(candidate, _)| candidate == provider) else {
            continue;
        };
        let rank = severity_rank(values.severity);
        if identity.is_none_or(|(best, _)| rank > best) {
            identity = Some((rank, values));
        }
    }
    if let Some((_, values)) = identity {
        for (name, value) in [
            ("quota_state", &values.quota_state),
            ("quota_provider", &values.quota_provider),
            ("quota_summary", &values.quota_summary),
        ] {
            desired.insert(name.to_string(), value.clone());
        }
        if let Some(error) = &values.quota_error {
            desired.insert("quota_error".to_string(), error.clone());
        }
    }

    for (provider, base) in [
        (Provider::Codex, "quota_week"),
        (Provider::Grok, "quota_5h"),
    ] {
        let Some((_, values)) = tokens.iter().find(|(candidate, _)| *candidate == provider) else {
            continue;
        };
        if values.quota_week.trim().is_empty() {
            continue;
        }
        // The prefix rides inside whichever severity variant gets selected.
        let value = format!("{} {}", provider.agent_kind(), values.quota_week);
        insert_optional_token(&mut desired, base, &value);
        insert_severity_token(&mut desired, base, &value, values.quota_week_severity);
    }

    insert_optional_token(&mut desired, "quota_topic", topic);
    desired
}

fn severity_rank(severity: crate::model::Severity) -> u8 {
    match severity {
        crate::model::Severity::Danger => 3,
        crate::model::Severity::Warning => 2,
        crate::model::Severity::Normal => 1,
        crate::model::Severity::Unknown => 0,
    }
}

fn display_topic(pane: &AgentPane) -> String {
    let topic = pane.topic.trim();
    if topic.is_empty() || is_status_line(topic) {
        return truncate_topic(&pane.session_summary);
    }
    truncate_topic(topic)
}

fn metadata_matches(
    current: &BTreeMap<String, String>,
    desired: &BTreeMap<String, String>,
) -> bool {
    METADATA_TOKEN_NAMES
        .into_iter()
        .all(|name| current.get(name) == desired.get(name))
        && OBSOLETE_METADATA_TOKEN_NAMES
            .into_iter()
            .all(|name| !current.contains_key(name))
        && LEGACY_METADATA_TOKEN_NAMES
            .into_iter()
            .all(|name| !current.contains_key(name))
}

fn metadata_report_names(
    pane: &AgentPane,
    desired: &BTreeMap<String, String>,
) -> Vec<&'static str> {
    let mut names = METADATA_TOKEN_NAMES
        .into_iter()
        .filter(|name| desired.contains_key(*name) || pane.tokens.contains_key(*name))
        .collect::<Vec<_>>();
    let cleanup_names = OBSOLETE_METADATA_TOKEN_NAMES
        .into_iter()
        .filter(|name| pane.tokens.contains_key(*name))
        .chain(
            LEGACY_METADATA_TOKEN_NAMES
                .into_iter()
                .filter(|name| pane.tokens.contains_key(*name)),
        )
        .collect::<Vec<_>>();
    if names.len() + cleanup_names.len() <= MAX_METADATA_TOKENS {
        names.extend(cleanup_names);
        return names;
    }

    // Herdr accepts at most sixteen token arguments. Reserve room for stale
    // names first so an upgraded pane can actually clear them; unchanged
    // cosmetic fields can be restored on the next bounded report.
    let active_capacity = MAX_METADATA_TOKENS.saturating_sub(cleanup_names.len());
    for candidate in ["quota_summary", "quota_state", "quota_5h", "quota_week"] {
        while names.len() > active_capacity {
            let Some(index) = names.iter().position(|name| {
                *name == candidate && pane.tokens.get(candidate) == desired.get(candidate)
            }) else {
                break;
            };
            names.remove(index);
        }
    }
    while names.len() > active_capacity {
        let Some(index) = names.iter().position(|name| {
            !matches!(
                *name,
                "quota_context"
                    | "quota_cache"
                    | "quota_cache_ttl"
                    | "quota_provider"
                    | "quota_topic"
            )
        }) else {
            break;
        };
        names.remove(index);
    }
    names.truncate(active_capacity);
    names.extend(cleanup_names);
    names
}

fn insert_severity_token(
    tokens: &mut BTreeMap<String, String>,
    base: &str,
    value: &str,
    severity: Option<crate::model::Severity>,
) {
    if value.trim().is_empty() {
        return;
    }
    let variant = severity_variant(severity);
    tokens.insert(format!("{base}_{variant}"), value.to_string());
}

fn severity_variant(severity: Option<crate::model::Severity>) -> &'static str {
    match severity.unwrap_or(crate::model::Severity::Unknown) {
        crate::model::Severity::Warning => "warning",
        crate::model::Severity::Danger => "danger",
        crate::model::Severity::Normal => "normal",
        crate::model::Severity::Unknown => "warning",
    }
}

fn insert_optional_token(tokens: &mut BTreeMap<String, String>, name: &str, value: &str) {
    if !value.trim().is_empty() {
        tokens.insert(name.to_string(), value.to_string());
    }
}

// `recent` rebuilds the pane's wrapped scrollback, which takes seconds and
// repaints the pane: the agent's terminal visibly scrolls, once per read.
// `visible` is the current screen only, costs microseconds, and repaints
// nothing. The prompt is on screen at the moment idle->working fires, which is
// exactly when the topic changes; later in the turn it may have scrolled off,
// and then the caller keeps the topic it already published.
fn topic_read_args(pane_id: &str) -> [&str; 7] {
    [
        "pane", "read", pane_id, "--source", "visible", "--format", "text",
    ]
}

fn read_pane_topic(
    executable: &std::ffi::OsStr,
    pane: &AgentPane,
    provider: &Provider,
) -> Option<String> {
    let output = Command::new(executable)
        .args(topic_read_args(&pane.pane_id))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    extract_topic(&text, *provider)
}

fn extract_topic(text: &str, provider: Provider) -> Option<String> {
    text.lines().rev().find_map(|line| {
        let cleaned_line = strip_control_chars(line);
        let line = cleaned_line.trim();
        let candidate = prompt_candidate(line, provider)?;
        if candidate.is_empty() || is_status_line(candidate) {
            return None;
        }
        Some(truncate_topic(candidate))
    })
}

fn prompt_candidate(line: &str, provider: Provider) -> Option<&str> {
    let marker = match provider {
        Provider::Claude if line.starts_with('❯') => '❯',
        Provider::Codex if line.starts_with('›') => '›',
        Provider::Grok if line.starts_with('❯') => '❯',
        Provider::Grok | Provider::Agy if line.starts_with('>') => '>',
        _ => return None,
    };
    Some(line.trim_start_matches(marker).trim())
}

fn truncate_topic(value: &str) -> String {
    let characters: Vec<char> = value.chars().collect();
    if characters.len() <= 80 {
        return value.to_string();
    }
    let mut topic: String = characters.into_iter().take(77).collect();
    topic.push('…');
    topic
}

fn strip_control_chars(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || *character == '\t')
        .collect()
}

fn is_status_line(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("accept-edits mode:")
        || lower.starts_with("context ")
        || lower.starts_with("session ")
        || lower.starts_with("auto mode")
        || lower.starts_with("shift+tab")
        || lower == "ask codex to do anything"
        || matches!(
            lower.as_str(),
            "/clear" | "/compact" | "/help" | "/status" | "/usage" | "/model" | "/config"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn discovers_canonical_agent_panes_from_nested_json() {
        let value = json!({"result": {"agents": [
            {"pane_id": "w1:p1", "tab_id": "w1:t1", "agent": "codex"},
            {"pane_id": "w1:p2", "tab_id": "w1:t2", "agent_session": {"agent": "claude"}},
            {"pane_id": "w1:p3", "agent": "unknown"}
        ], "tabs": [
            {"tab_id": "w1:t1", "label": "Owner"},
            {"tab_id": "w1:t2", "label": "Executor"}
        ]}});
        let mut panes = Vec::new();
        collect_agent_panes(&value, &mut panes);
        panes.sort_by(|left, right| left.pane_id.cmp(&right.pane_id));
        assert_eq!(
            panes,
            vec![
                AgentPane {
                    pane_id: "w1:p1".to_string(),
                    providers: vec![Provider::Codex],
                    session_id: None,
                    session_summary: String::new(),
                    topic: String::new(),
                    tokens: BTreeMap::new(),
                },
                AgentPane {
                    pane_id: "w1:p2".to_string(),
                    providers: vec![Provider::Claude],
                    session_id: None,
                    session_summary: String::new(),
                    topic: String::new(),
                    tokens: BTreeMap::new(),
                },
            ]
        );
    }

    #[test]
    fn quota_only_discovery_preserves_the_last_published_topic() {
        let value = json!({"result": {"agents": [{
            "pane_id": "w1:p1",
            "agent": "grok",
            "tokens": {"quota_topic": "latest task"}
        }]}});
        let mut panes = Vec::new();
        collect_agent_panes(&value, &mut panes);
        assert_eq!(panes[0].topic, "latest task");
    }

    #[test]
    fn opencode_panes_collect_both_subscription_providers() {
        let value = json!({"result": {"agents": [
            {"pane_id": "w1:p9", "agent": "opencode"},
            {"pane_id": "w1:p1", "agent": "codex"}
        ]}});
        let mut panes = Vec::new();
        collect_agent_panes(&value, &mut panes);
        panes.sort_by(|left, right| left.pane_id.cmp(&right.pane_id));
        assert_eq!(panes[0].providers, vec![Provider::Codex]);
        assert_eq!(panes[1].providers, vec![Provider::Codex, Provider::Grok]);
    }

    #[test]
    fn discovers_codex_session_id_and_preserves_session_summary() {
        let value = json!({"result": {"agents": [{
            "pane_id": "w1:p1",
            "agent": "codex",
            "agent_session": {"agent": "codex", "value": "thread-1"},
            "tokens": {"quota_session": "previous summary"}
        }]}});
        let mut panes = Vec::new();
        collect_agent_panes(&value, &mut panes);
        assert_eq!(panes[0].session_id.as_deref(), Some("thread-1"));
        assert_eq!(panes[0].session_summary, "previous summary");
    }

    #[test]
    fn legacy_metadata_tokens_force_one_bounded_cleanup_report() {
        let pane = AgentPane {
            pane_id: "w1:p1".to_string(),
            providers: vec![Provider::Claude],
            session_id: None,
            session_summary: String::new(),
            topic: String::new(),
            tokens: BTreeMap::from([(String::from("quota_badge"), String::from("[A]"))]),
        };
        let desired = BTreeMap::from([(String::from("quota_state"), String::from("?"))]);
        assert!(!metadata_matches(&pane.tokens, &desired));
        let names = metadata_report_names(&pane, &desired);
        assert!(names.contains(&"quota_badge"));
        assert!(names.len() <= MAX_METADATA_TOKENS);
    }

    #[test]
    fn cache_diagnostics_stay_inside_herdr_metadata_cap() {
        let snapshot = crate::model::ProviderSnapshot::new(
            Provider::Claude,
            vec![
                crate::model::UsageWindow::new(crate::model::WindowKind::FiveHour, 20.0, None)
                    .unwrap(),
                crate::model::UsageWindow::new(crate::model::WindowKind::Weekly, 30.0, None)
                    .unwrap(),
            ],
            0,
        )
        .with_context(Some(
            crate::model::ContextUsage::new(42.0)
                .unwrap()
                .with_cache(Some(
                    crate::model::CacheUsage::from_token_counts(100, 800, 100)
                        .unwrap()
                        .with_ttl_estimate(3_600, 0)
                        .with_session_totals(
                            crate::model::CacheTotals::from_token_counts(100, 800, 100),
                            "session-1",
                            1,
                        ),
                )),
        ));
        let desired = desired_tokens(&MetadataTokens::from_snapshot(&snapshot, 0), "prompt");
        let pane = AgentPane {
            pane_id: "w1:p1".to_string(),
            providers: vec![Provider::Claude],
            session_id: None,
            session_summary: String::new(),
            topic: String::new(),
            tokens: BTreeMap::new(),
        };
        let names = metadata_report_names(&pane, &desired);
        assert!(names.len() <= MAX_METADATA_TOKENS);
        assert!(names.contains(&"quota_cache"));
        assert!(names.contains(&"quota_cache_ttl"));
    }

    #[test]
    fn stale_metadata_tokens_are_reported_for_cleanup_with_new_cache_rows() {
        let snapshot = crate::model::ProviderSnapshot::new(
            Provider::Claude,
            vec![
                crate::model::UsageWindow::new(crate::model::WindowKind::FiveHour, 20.0, None)
                    .unwrap(),
                crate::model::UsageWindow::new(crate::model::WindowKind::Weekly, 30.0, None)
                    .unwrap(),
            ],
            0,
        )
        .with_context(Some(
            crate::model::ContextUsage::new(42.0)
                .unwrap()
                .with_cache(Some(
                    crate::model::CacheUsage::from_token_counts(100, 800, 100)
                        .unwrap()
                        .with_ttl_estimate(3_600, 0)
                        .with_session_totals(
                            crate::model::CacheTotals::from_token_counts(100, 800, 100),
                            "session-1",
                            1,
                        ),
                )),
        ));
        let desired = desired_tokens(&MetadataTokens::from_snapshot(&snapshot, 0), "prompt");
        let mut tokens = desired.clone();
        tokens.insert("quota_icon".to_string(), "✦Cl".to_string());
        tokens.insert("quota_status".to_string(), "OK".to_string());
        tokens.insert("quota_badge".to_string(), "[C]".to_string());
        tokens.insert("quota_session".to_string(), "old".to_string());
        let pane = AgentPane {
            pane_id: "w1:p1".to_string(),
            providers: vec![Provider::Claude],
            session_id: None,
            session_summary: String::new(),
            topic: String::new(),
            tokens,
        };
        let names = metadata_report_names(&pane, &desired);
        assert!(names.len() <= MAX_METADATA_TOKENS);
        assert!(names.contains(&"quota_cache"));
        assert!(names.contains(&"quota_cache_ttl"));
        assert!(names.contains(&"quota_icon"));
        assert!(names.contains(&"quota_status"));
        assert!(names.contains(&"quota_badge"));
        assert!(names.contains(&"quota_session"));
    }

    #[test]
    fn working_agent_detection_handles_herdr_agent_list_shape() {
        let value = json!({"result": {"agents": [
            {"agent": "claude", "agent_status": "working"},
            {"agent": "codex", "agent_status": "idle"}
        ]}});
        assert_eq!(working_providers_from(&value), vec![Provider::Claude]);
    }

    #[test]
    fn one_agent_inventory_deduplicates_working_providers() {
        let value = json!({"result": {"agents": [
            {"agent": "codex", "agent_status": "working"},
            {"agent_session": {"agent": "codex"}, "status": "working"},
            {"agent": "claude", "agent_status": "idle"}
        ]}});
        assert_eq!(working_providers_from(&value), vec![Provider::Codex]);
    }

    #[test]
    fn extracts_latest_agy_prompt_instead_of_status_line() {
        let text = "> older\nHello\n> hi\nHello!\n> Accept-edits mode: file edits auto-approved\n";
        assert_eq!(extract_topic(text, Provider::Agy).as_deref(), Some("hi"));
    }

    #[test]
    fn extracts_latest_claude_prompt_and_skips_clear_command() {
        let text = "❯ /clear\n❯ hi\n⏺ Hi! What can I help with?\n❯\n";
        assert_eq!(extract_topic(text, Provider::Claude).as_deref(), Some("hi"));
    }

    #[test]
    fn ignores_codex_default_prompt_placeholder() {
        assert_eq!(
            extract_topic("› Ask Codex to do anything\n", Provider::Codex),
            None
        );
    }

    #[test]
    fn ignores_ai_status_title_as_a_topic() {
        let value = json!({
            "pane_id": "w1:p1",
            "agent": "grok",
            "terminal_title_stripped": "Thinking - L7 Learning Reset"
        });
        let mut panes = Vec::new();
        collect_agent_panes(&value, &mut panes);
        assert_eq!(panes[0].topic, "");
    }

    #[test]
    fn publishes_exactly_one_styled_variant_for_each_window() {
        let mut tokens = BTreeMap::new();
        insert_severity_token(
            &mut tokens,
            "quota_week",
            "week 25% reset 2d3h",
            Some(crate::model::Severity::Warning),
        );
        assert_eq!(
            tokens.get("quota_week_warning").map(String::as_str),
            Some("week 25% reset 2d3h")
        );
        assert!(!tokens.contains_key("quota_week_normal"));
        assert!(!tokens.contains_key("quota_week_danger"));
    }

    #[test]
    fn extracts_latest_grok_user_prompt_instead_of_ai_output() {
        let text = "❯ /goal 你在 ti 工作区接手 L7\n先读计划与权威文档，再按七步做 L7 盘点与设计。\n◇ Ran 1 subagent\n计划已读。先冻结坐标并读材料。\n";
        assert_eq!(
            extract_topic(text, Provider::Grok).as_deref(),
            Some("/goal 你在 ti 工作区接手 L7")
        );
    }

    // `recent` and `recent-unwrapped` rebuild the pane's wrapped scrollback,
    // which repaints it: one read, one visible scroll for the user.
    #[test]
    fn topic_reads_never_rebuild_a_pane_scrollback() {
        let args = topic_read_args("w1:p1");
        assert!(args.contains(&"visible"));
        assert!(!args.contains(&"recent"));
        assert!(!args.contains(&"recent-unwrapped"));
    }

    #[test]
    fn truncates_topics_without_splitting_utf8() {
        let topic = truncate_topic(&"你好".repeat(50));
        assert!(topic.ends_with('…'));
        assert!(topic.chars().count() <= 78);
    }

    fn weekly_tokens(
        provider: Provider,
        used_percent: f64,
        reset_in_seconds: u64,
    ) -> MetadataTokens {
        let now = 1_000_000;
        let snapshot = crate::model::ProviderSnapshot::new(
            provider,
            vec![crate::model::UsageWindow::new(
                crate::model::WindowKind::Weekly,
                used_percent,
                Some(crate::model::ResetAt::after(now, reset_in_seconds)),
            )
            .unwrap()],
            now,
        );
        MetadataTokens::from_snapshot(&snapshot, now)
    }

    // Codex and Grok only ever carry a weekly window, so an opencode card
    // reuses the always-absent $quota_5h_* slots for Grok's week while Codex
    // keeps $quota_week_*. Severities are computed per provider, and each
    // value carries its provider's lowercase kind name so the rows read
    // unambiguously on the shared card.
    #[test]
    fn opencode_cards_place_each_subscription_week_in_distinct_groups() {
        let providers = Provider::providers_for_agent("opencode").unwrap();
        let tokens = vec![
            (
                Provider::Codex,
                weekly_tokens(Provider::Codex, 25.0, 1_209_600),
            ),
            (Provider::Grok, weekly_tokens(Provider::Grok, 60.0, 60_480)),
        ];
        let desired = desired_multi_provider_tokens(&providers, &tokens, "");
        assert_eq!(
            desired.get("quota_week").map(String::as_str),
            Some("codex week 75% reset 14d0h")
        );
        assert_eq!(
            desired.get("quota_week_warning").map(String::as_str),
            Some("codex week 75% reset 14d0h")
        );
        assert!(!desired.contains_key("quota_week_normal"));
        assert!(!desired.contains_key("quota_week_danger"));
        assert_eq!(
            desired.get("quota_5h").map(String::as_str),
            Some("grok week 40% reset 16h48m")
        );
        assert_eq!(
            desired.get("quota_5h_normal").map(String::as_str),
            Some("grok week 40% reset 16h48m")
        );
        assert!(!desired.contains_key("quota_5h_warning"));
        assert!(!desired.contains_key("quota_5h_danger"));
    }

    #[test]
    fn opencode_identity_slots_follow_worst_severity_and_prefer_codex_on_ties() {
        let providers = Provider::providers_for_agent("opencode").unwrap();

        // Equal severities keep Codex, the first provider for the card.
        let tokens = vec![
            (
                Provider::Codex,
                weekly_tokens(Provider::Codex, 85.0, 60_480),
            ),
            (Provider::Grok, weekly_tokens(Provider::Grok, 60.0, 60_480)),
        ];
        let desired = desired_multi_provider_tokens(&providers, &tokens, "");
        assert_eq!(
            desired.get("quota_provider").map(String::as_str),
            Some("Codex")
        );
        assert_eq!(desired.get("quota_state").map(String::as_str), Some("●"));
        assert!(!desired.contains_key("quota_badge"));
        assert!(!desired.contains_key("quota_status"));

        // A worse severity on Grok takes the identity slots over.
        let tokens = vec![
            (
                Provider::Codex,
                weekly_tokens(Provider::Codex, 85.0, 60_480),
            ),
            (
                Provider::Grok,
                weekly_tokens(Provider::Grok, 95.0, 1_209_600),
            ),
        ];
        let desired = desired_multi_provider_tokens(&providers, &tokens, "");
        assert_eq!(
            desired.get("quota_provider").map(String::as_str),
            Some("Grok")
        );
        assert_eq!(desired.get("quota_state").map(String::as_str), Some("!"));
        assert!(!desired.contains_key("quota_badge"));
        assert!(!desired.contains_key("quota_status"));

        // With no Grok data at all the card still renders Codex alone.
        let codex_only = vec![(
            Provider::Codex,
            weekly_tokens(Provider::Codex, 85.0, 60_480),
        )];
        let desired = desired_multi_provider_tokens(&providers, &codex_only, "topic");
        assert_eq!(
            desired.get("quota_provider").map(String::as_str),
            Some("Codex")
        );
        assert!(desired.contains_key("quota_week_normal"));
        assert!(!desired.contains_key("quota_5h"));
    }

    // Regression guard: values on single-provider panes stay bare; only the
    // merged-card composer adds the provider kind prefix.
    #[test]
    fn single_provider_windows_stay_unprefixed() {
        let desired = desired_tokens(&weekly_tokens(Provider::Codex, 25.0, 1_209_600), "");
        assert_eq!(
            desired.get("quota_week").map(String::as_str),
            Some("week 75% reset 14d0h")
        );
        assert_eq!(
            desired.get("quota_week_warning").map(String::as_str),
            Some("week 75% reset 14d0h")
        );
        assert!(!desired.contains_key("quota_5h"));
    }
}
