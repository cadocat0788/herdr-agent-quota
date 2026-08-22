use herdr_agent_quota::configure::herdr::{add_quota_row, remove_quota_row};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::tempdir;

fn install_herdr_stub(state: &Path, agent_list: &str) -> (PathBuf, PathBuf) {
    let log = state.join("herdr.log");
    let executable = state.join("herdr");
    fs::write(
        &executable,
        format!(
            "#!/bin/sh\nif [ \"$1 $2\" = \"agent list\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1 $2\" = \"pane read\" ]; then\n  printf '%s\\n' \"$*\" >> '{}'\nelif [ \"$1 $2\" = \"pane report-metadata\" ]; then\n  printf '%s\\n' \"$*\" >> '{}'\nfi\n",
            agent_list,
            log.display(),
            log.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).unwrap();
    (executable, log)
}

fn run_claude_collector(state: &Path, herdr: &Path, input: &[u8]) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .arg("claude-statusline")
        .env("HERDR_PLUGIN_STATE_DIR", state)
        .env("HERDR_BIN_PATH", herdr)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
}

fn run_claude_refresh(state: &Path, herdr: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .args(["refresh", "--provider", "claude", "--force"])
        .env("HERDR_PLUGIN_STATE_DIR", state)
        .env("HERDR_BIN_PATH", herdr)
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn sidebar_configuration_is_idempotent_and_reversible() {
    let original = "[ui.sidebar.agents]\nrows = [[\"state_icon\", \"agent\"]]\n";
    let applied = add_quota_row(original).unwrap();
    assert!(applied.contains("key = \"prefix+shift+r\""));
    assert!(applied.contains("type = \"plugin_action\""));
    assert!(applied.contains("command = \"herdr-agent-quota.refresh\""));
    assert_eq!(add_quota_row(&applied).unwrap(), applied);
    assert_eq!(remove_quota_row(&applied).unwrap(), original);
}

#[test]
fn sidebar_configuration_preserves_a_conflicting_refresh_key() {
    let original = concat!(
        "[[keys.command]]\n",
        "key = \"prefix+shift+r\"\n",
        "type = \"shell\"\n",
        "command = \"echo user-owned\"\n",
        "description = \"user refresh\"\n\n",
        "[ui.sidebar.agents]\n",
        "rows = [[\"state_icon\", \"agent\"]]\n"
    );
    let applied = add_quota_row(original).unwrap();
    assert_eq!(applied.matches("key = \"prefix+shift+r\"").count(), 1);
    assert!(applied.contains("command = \"echo user-owned\""));
    assert!(!applied.contains("command = \"herdr-agent-quota.refresh\""));
    assert_eq!(remove_quota_row(&applied).unwrap(), original);
}

#[test]
fn default_herdr_rows_become_plane_provider_usage_and_topic_lines() {
    let original = concat!(
        "[ui.sidebar.agents]\n",
        "rows = [[\"state_icon\", \"workspace\", \"tab\"], [\"agent\"]]\n"
    );
    let applied = add_quota_row(original).unwrap();
    assert!(applied.contains("$quota_provider"));
    assert!(applied.contains("bold = true"));
    assert!(applied.contains("$quota_5h_normal"));
    assert!(applied.contains("$quota_5h_warning"));
    assert!(applied.contains("$quota_5h_danger"));
    assert!(applied.contains("$quota_week_normal"));
    assert!(applied.contains("$quota_week_warning"));
    assert!(applied.contains("$quota_week_danger"));
    assert!(!applied.contains("[\"$quota_summary\"]"));
    assert!(applied.contains("$quota_topic"));
    assert!(applied.contains("row_gap = 1 # herdr-agent-quota"));
    assert!(applied.find("$quota_topic").unwrap() < applied.find("$quota_5h_normal").unwrap());
    assert!(applied.contains("fg = \"#84b084\""));
    assert!(applied.contains("fg = \"#cdaa65\""));
    assert!(applied.contains("fg = \"#ca6470\""));
    assert!(applied.contains("[ui.sidebar.agents.rows_by_agent]"));
    assert!(applied.contains("fg = \"#c47f6a\""));
    assert!(applied.contains("fg = \"#7998b7\""));
    assert!(applied.contains("fg = \"#acb4c3\""));
    assert!(applied.contains("fg = \"#84b0af\""));
}

#[test]
fn sidebar_configuration_preserves_an_explicit_row_gap() {
    let original = concat!(
        "[ui.sidebar.agents]\n",
        "row_gap = 2\n",
        "rows = [[\"state_icon\", \"agent\"]]\n"
    );
    let applied = add_quota_row(original).unwrap();
    assert!(applied.contains("row_gap = 2"));
    assert!(!applied.contains("row_gap = 1"));
    assert_eq!(remove_quota_row(&applied).unwrap(), original);
}

#[test]
fn claude_collector_is_silent_without_a_previous_statusline() {
    let state = tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .arg("claude-statusline")
        .env("HERDR_PLUGIN_STATE_DIR", state.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("fixtures/claude/statusline-both.json"))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn agy_collector_is_silent_without_a_previous_statusline() {
    let state = tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .arg("agy-statusline")
        .env("HERDR_PLUGIN_STATE_DIR", state.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(include_bytes!("fixtures/agy/statusline-both.json"))
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn claude_cache_is_published_by_refresh_event() {
    let state = tempdir().unwrap();
    let (herdr_stub, herdr_log) = install_herdr_stub(
        state.path(),
        r#"{"result":{"agents":[{"agent":"claude","pane_id":"w1:p1"}]}}"#,
    );
    run_claude_collector(
        state.path(),
        &herdr_stub,
        include_bytes!("fixtures/claude/statusline-both.json"),
    );
    assert!(!herdr_log.exists());

    run_claude_refresh(state.path(), &herdr_stub);
    let report = fs::read_to_string(herdr_log).unwrap();
    assert!(!report.contains("pane read"));
    // Single-provider panes publish bare values; the kind prefix only ever
    // appears on multi-provider cards.
    assert!(report.contains("quota_5h=5h 42% reset"));
    assert!(report.contains("quota_week=week 73% reset"));
    assert!(!report.contains("quota_week=claude"));
}

#[test]
fn quota_refresh_does_not_report_metadata_to_a_scrolled_pane() {
    let state = tempdir().unwrap();
    let log = state.path().join("herdr.log");
    let herdr = state.path().join("herdr");
    fs::write(
        &herdr,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = \"agent list\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1 $2\" = \"pane get\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
            log.display(),
            r#"{"result":{"agents":[{"agent":"claude","pane_id":"w1:p1"}]}}"#,
            r#"{"result":{"pane":{"scroll":{"offset_from_bottom":12}}}}"#,
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&herdr).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&herdr, permissions).unwrap();

    run_claude_collector(
        state.path(),
        &herdr,
        include_bytes!("fixtures/claude/statusline-both.json"),
    );
    run_claude_refresh(state.path(), &herdr);
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("pane get w1:p1"));
    assert!(!calls.contains("pane report-metadata"));
}

#[test]
fn focus_refreshes_only_the_selected_provider_without_reading_the_pane() {
    let state = tempdir().unwrap();
    let log = state.path().join("herdr.log");
    let herdr = state.path().join("herdr");
    fs::write(
        &herdr,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = \"pane current\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1 $2\" = \"agent list\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1 $2\" = \"pane get\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
            log.display(),
            r#"{"result":{"pane":{"agent":"claude","pane_id":"w1:p1"}}}"#,
            r#"{"result":{"agents":[{"agent":"claude","pane_id":"w1:p1"}]}}"#,
            r#"{"result":{"pane":{"scroll":{"offset_from_bottom":0}}}}"#,
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&herdr).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&herdr, permissions).unwrap();
    run_claude_collector(
        state.path(),
        &herdr,
        include_bytes!("fixtures/claude/statusline-both.json"),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .arg("focus")
        .env("HERDR_PLUGIN_STATE_DIR", state.path())
        .env("HERDR_BIN_PATH", &herdr)
        .output()
        .unwrap();
    assert!(output.status.success());
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("pane current"));
    assert!(!calls.contains("pane read"));
    assert!(calls.contains("pane report-metadata w1:p1"));
}

#[test]
fn agent_event_refreshes_and_reads_topics_only_for_its_provider() {
    let state = tempdir().unwrap();
    let log = state.path().join("herdr.log");
    let codex_log = state.path().join("codex.log");
    let herdr = state.path().join("herdr");
    let codex = state.path().join("codex");
    fs::write(
        &herdr,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = \"agent list\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1 $2\" = \"pane get\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
            log.display(),
            r#"{"result":{"agents":[{"agent":"claude","pane_id":"w1:p1"},{"agent":"grok","pane_id":"w1:p2"}]}}"#,
            r#"{"result":{"pane":{"scroll":{"offset_from_bottom":0}}}}"#,
        ),
    )
    .unwrap();
    fs::write(
        &codex,
        format!("#!/bin/sh\nprintf called > '{}'\n", codex_log.display()),
    )
    .unwrap();
    for executable in [&herdr, &codex] {
        let mut permissions = fs::metadata(executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(executable, permissions).unwrap();
    }
    run_claude_collector(
        state.path(),
        &herdr,
        include_bytes!("fixtures/claude/statusline-both.json"),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .arg("event")
        .env("HERDR_PLUGIN_STATE_DIR", state.path())
        .env("HERDR_BIN_PATH", &herdr)
        .env("CODEX_BIN_PATH", &codex)
        .env("GROK_HOME", state.path().join("missing-grok-home"))
        .env(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"event":{"pane":{"agent":"claude","pane_id":"w1:p1"}}}"#,
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!codex_log.exists());
    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("pane read w1:p1"));
    assert!(!calls.contains("pane read w1:p2"));
}

#[test]
fn claude_collector_does_not_republish_unchanged_quota() {
    let state = tempdir().unwrap();
    let (herdr_stub, herdr_log) = install_herdr_stub(
        state.path(),
        r#"{"result":{"agents":[{"agent":"claude","pane_id":"w1:p1","tokens":{"quota_badge":"[A]","quota_state":"?","quota_icon":"✦Cl","quota_provider":"Claude","quota_status":"N/A","quota_5h":"5h 42%","quota_5h_warning":"5h 42%","quota_week":"week 73%","quota_week_warning":"week 73%","quota_summary":"5h 42% · week 73%"}}]}}"#,
    );

    let input = br#"{
        "rate_limits": {
            "five_hour": {"used_percentage": 58.0},
            "seven_day": {"used_percentage": 27.0}
        }
    }"#;
    run_claude_collector(state.path(), &herdr_stub, input);
    assert!(!herdr_log.exists());

    run_claude_refresh(state.path(), &herdr_stub);
    assert!(!herdr_log.exists());
}

fn seed_weekly_snapshot(state: &Path, source: &str, provider: &str, remaining_percent: f64) {
    let payload = format!(
        r#"{{"provider":"{provider}","source":"{source}","fetched_at_unix":1750000000,"windows":[{{"kind":"weekly","used_percent":{used},"remaining_percent":{remaining},"resets_at":4102444800}}]}}"#,
        used = 100.0 - remaining_percent,
        remaining = remaining_percent,
    );
    fs::write(state.join(format!("{source}.json")), payload).unwrap();
}

#[test]
fn opencode_event_publishes_both_subscription_windows_without_reading_the_pane() {
    let state = tempdir().unwrap();
    let log = state.path().join("herdr.log");
    let herdr = state.path().join("herdr");
    fs::write(
        &herdr,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1 $2\" = \"agent list\" ]; then\n  printf '%s\\n' '{}'\nelif [ \"$1 $2\" = \"pane get\" ]; then\n  printf '%s\\n' '{}'\nfi\n",
            log.display(),
            r#"{"result":{"agents":[{"agent":"opencode","pane_id":"w1:p9"}]}}"#,
            r#"{"result":{"pane":{"scroll":{"offset_from_bottom":0}}}}"#,
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&herdr).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&herdr, permissions).unwrap();

    seed_weekly_snapshot(state.path(), "codex-app-server", "codex", 75.0);
    seed_weekly_snapshot(state.path(), "grok-cli-billing", "grok", 40.0);

    let output = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"))
        .arg("event")
        .env("HERDR_PLUGIN_STATE_DIR", state.path())
        .env("HERDR_BIN_PATH", &herdr)
        .env("CODEX_BIN_PATH", state.path().join("missing-codex"))
        .env("GROK_HOME", state.path().join("missing-grok-home"))
        .env(
            "HERDR_PLUGIN_EVENT_JSON",
            r#"{"event":{"type":"pane_agent_detected","pane":{"agent":"opencode","pane_id":"w1:p9"}}}"#,
        )
        .output()
        .unwrap();
    assert!(output.status.success());

    let calls = fs::read_to_string(log).unwrap();
    assert!(calls.contains("agent list"));
    // Multi-agent cards mix prompt styles, so no pane is ever read for them.
    assert!(!calls.contains("pane read"));
    assert!(calls.contains("pane report-metadata w1:p9"));
    // Codex owns the weekly group; Grok reuses the five-hour slots for its
    // weekly window. Each value names its provider so the shared card's rows
    // are distinguishable.
    assert!(calls.contains("--token quota_week=codex week 75% reset "));
    assert!(calls.contains("--token quota_5h=grok week 40% reset "));
    // Both runways are far from their reset, so both windows warn; the shared
    // identity stays Codex because equal severities prefer it.
    assert!(calls.contains("--token quota_week_warning=codex week 75%"));
    assert!(calls.contains("--token quota_5h_warning=grok week 40%"));
    assert!(calls.contains("--token quota_provider=Codex"));
    assert!(calls.contains("--token quota_badge=[C]"));
}

#[test]
fn configure_writes_and_removes_a_managed_opencode_sidebar_entry() {
    let state = tempdir().unwrap();
    let config = state.path().join("config.toml");
    let claude_settings = state.path().join("claude-settings.json");
    let agy_settings = state.path().join("agy-settings.json");
    // Keep every collector's settings inside the tempdir; configure --apply
    // installs statusline hooks and Grok refresh hooks beyond Herdr's config.
    let isolated = |command: &mut Command| {
        command
            .env("HERDR_PLUGIN_STATE_DIR", state.path())
            .env("HERDR_CONFIG_FILE", &config)
            .env("CLAUDE_SETTINGS_FILE", &claude_settings)
            .env("AGY_SETTINGS_FILE", &agy_settings)
            .env("GROK_HOME", state.path().join("grok"));
    };

    fs::write(
        &config,
        "[ui.sidebar.agents]\nrows = [[\"state_icon\", \"workspace\", \"tab\"], [\"agent\"]]\n",
    )
    .unwrap();

    let mut applied = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"));
    isolated(&mut applied);
    let applied = applied.arg("configure").arg("--apply").output().unwrap();
    assert!(applied.status.success());
    let content = fs::read_to_string(&config).unwrap();
    assert!(content.contains("[ui.sidebar.agents.rows_by_agent]"));
    assert!(content.contains("opencode = [["));
    assert!(content.contains("#7a9e7e"));
    assert_eq!(
        add_quota_row(&content).unwrap(),
        content,
        "applying again must be idempotent"
    );

    let mut removed = Command::new(env!("CARGO_BIN_EXE_herdr-agent-quota"));
    isolated(&mut removed);
    let removed = removed
        .arg("configure")
        .arg("--uninstall")
        .output()
        .unwrap();
    assert!(removed.status.success());
    let restored = fs::read_to_string(&config).unwrap();
    assert!(!restored.contains("opencode = [["));
    // apply normalizes the official row, mapping workspace back to tab.
    assert!(restored.contains("[[\"state_icon\", \"tab\"]]"));
}
