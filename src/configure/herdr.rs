use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value};

const QUOTA_ROW_MARKERS: [&str; 18] = [
    "$quota_badge",
    "$quota_state",
    "$quota_icon",
    "$quota_provider",
    "$quota_status",
    "$quota_summary",
    "$quota_topic",
    "$quota_5h",
    "$quota_week",
    "$quota_header",
    "$quota_5h_normal",
    "$quota_5h_warning",
    "$quota_5h_danger",
    "$quota_5h_unknown",
    "$quota_week_normal",
    "$quota_week_warning",
    "$quota_week_danger",
    "$quota_week_unknown",
];
const ROW_GAP_MARKER: &str = "herdr-agent-quota";
const PROVIDER_STYLE_MARKER: &str = "herdr-agent-quota-provider";
const REFRESH_KEY: &str = "prefix+shift+r";
const REFRESH_ACTION: &str = "herdr-agent-quota.refresh";
const QUOTA_SAFE_COLOR: &str = "#84b084";
const QUOTA_WARNING_COLOR: &str = "#cdaa65";
const QUOTA_DANGER_COLOR: &str = "#ca6470";
// `opencode` is a pane kind, not a data source, and deliberately gets no new
// metadata token names: its card reuses the existing $quota_* slots, which
// keeps the token diff-gate in src/herdr.rs untouched.
const PROVIDER_STYLES: [(&str, Option<&str>); 5] = [
    ("claude", Some("#c47f6a")),
    ("codex", Some("#7998b7")),
    ("grok", Some("#acb4c3")),
    ("agy", Some("#84b0af")),
    ("opencode", Some("#7a9e7e")),
];

pub fn check() -> Result<()> {
    let path = config_path()?;
    let original = fs::read_to_string(&path).unwrap_or_default();
    let updated = add_quota_row(&original)?;
    if updated == original {
        println!(
            "Herdr sidebar already contains quota tokens: {}",
            path.display()
        );
    } else {
        println!("Herdr sidebar preview for {}:", path.display());
        print_diff_hint();
    }
    Ok(())
}

pub fn apply() -> Result<()> {
    let path = config_path()?;
    let original = fs::read_to_string(&path).unwrap_or_default();
    let updated = add_quota_row(&original)?;
    if updated == original {
        return Ok(());
    }
    if !original.is_empty() {
        if let Some(backup) = backup_path()? {
            if let Some(parent) = backup.parent() {
                fs::create_dir_all(parent).context("create plugin state directory")?;
            }
            if !backup.exists() {
                fs::write(backup, &original).context("write Herdr config backup")?;
            }
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create Herdr config directory")?;
    }
    fs::write(&path, updated).context("write Herdr config")?;
    println!("Added quota sidebar row to {}", path.display());
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(());
    }
    let original = fs::read_to_string(&path).context("read Herdr config")?;
    let updated = remove_quota_row(&original)?;
    if updated != original {
        fs::write(&path, updated).context("remove quota sidebar row")?;
        println!("Removed quota sidebar row from {}", path.display());
    }
    if let Some(backup) = backup_path()? {
        if backup.exists() {
            fs::remove_file(backup).context("remove Herdr config backup")?;
        }
    }
    Ok(())
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_CONFIG_FILE") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/herdr/config.toml"))
}

fn backup_path() -> Result<Option<PathBuf>> {
    let state = std::env::var_os("HERDR_PLUGIN_STATE_DIR");
    Ok(state.map(|directory| PathBuf::from(directory).join("herdr-config.original.toml")))
}

pub fn add_quota_row(input: &str) -> Result<String> {
    let mut document = if input.trim().is_empty() {
        DocumentMut::new()
    } else {
        input
            .parse::<DocumentMut>()
            .context("parse Herdr TOML config")?
    };
    add_refresh_keybinding(&mut document)?;
    let agents = ensure_table(&mut document, &["ui", "sidebar", "agents"])?;
    if !agents.contains_key("row_gap") {
        let mut row_gap = Value::from(1);
        row_gap
            .decor_mut()
            .set_suffix(format!(" # {ROW_GAP_MARKER}"));
        agents.insert("row_gap", Item::Value(row_gap));
    }
    let rows = agents["rows"].or_insert(Item::Value(Value::Array(Array::new())));
    let rows = rows
        .as_array_mut()
        .context("Herdr ui.sidebar.agents.rows must be an array")?;
    let mut updated_rows = Array::new();
    for row in rows.iter() {
        let cleaned = normalize_official_row(strip_quota_tokens(row));
        if !cleaned.is_empty() {
            updated_rows.push(Value::Array(cleaned));
        }
    }

    // If an older version replaced every row with quota-only rows, restore
    // Herdr's official state/tab row before adding provider, usage, and topic.
    if updated_rows.is_empty() {
        updated_rows.push(Value::Array(default_state_row()));
    }
    append_quota_rows(&mut updated_rows);
    *rows = updated_rows;
    let rows = rows.clone();
    add_provider_rows(agents, &rows)?;
    Ok(document.to_string())
}

pub fn remove_quota_row(input: &str) -> Result<String> {
    if input.trim().is_empty() {
        return Ok(input.to_string());
    }
    let mut document = input
        .parse::<DocumentMut>()
        .context("parse Herdr TOML config")?;
    remove_refresh_keybinding(&mut document);
    let Some(agents) = document
        .get_mut("ui")
        .and_then(Item::as_table_mut)
        .and_then(|ui| ui.get_mut("sidebar"))
        .and_then(Item::as_table_mut)
        .and_then(|sidebar| sidebar.get_mut("agents"))
        .and_then(Item::as_table_mut)
    else {
        return Ok(document.to_string());
    };
    remove_managed_provider_rows(agents);
    if let Some(rows) = agents.get_mut("rows").and_then(Item::as_array_mut) {
        let mut retained = Array::new();
        for row in rows.iter() {
            let cleaned = strip_quota_tokens(row);
            if cleaned.len() == 1
                && matches!(
                    cleaned.iter().next().and_then(Value::as_str),
                    Some("terminal_title_stripped") | Some("$quota_topic")
                )
            {
                continue;
            }
            if !cleaned.is_empty() {
                retained.push(Value::Array(cleaned));
            }
        }
        agents["rows"] = Item::Value(Value::Array(retained));
    }
    let managed_row_gap = agents
        .get("row_gap")
        .and_then(Item::as_value)
        .and_then(|value| value.decor().suffix())
        .and_then(|suffix| suffix.as_str())
        .is_some_and(|suffix| suffix.contains(ROW_GAP_MARKER));
    if managed_row_gap {
        agents.remove("row_gap");
    }
    Ok(document.to_string())
}

fn add_refresh_keybinding(document: &mut DocumentMut) -> Result<()> {
    let keys = ensure_table(document, &["keys"])?;
    let commands = keys
        .entry("command")
        .or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .context("Herdr keys.command must be an array of tables")?;
    if commands
        .iter()
        .any(|command| command.get("command").and_then(Item::as_str) == Some(REFRESH_ACTION))
        || commands
            .iter()
            .any(|command| command.get("key").and_then(Item::as_str) == Some(REFRESH_KEY))
    {
        return Ok(());
    }

    let mut command = Table::new();
    command.insert("key", Item::Value(Value::from(REFRESH_KEY)));
    command.insert("type", Item::Value(Value::from("plugin_action")));
    command.insert("command", Item::Value(Value::from(REFRESH_ACTION)));
    command.insert(
        "description",
        Item::Value(Value::from("refresh all agent quotas")),
    );
    commands.push(command);
    Ok(())
}

fn remove_refresh_keybinding(document: &mut DocumentMut) {
    let Some(keys) = document.get_mut("keys").and_then(Item::as_table_mut) else {
        return;
    };
    let Some(commands) = keys
        .get_mut("command")
        .and_then(Item::as_array_of_tables_mut)
    else {
        return;
    };
    let mut retained = ArrayOfTables::new();
    for command in commands.iter() {
        if command.get("command").and_then(Item::as_str) != Some(REFRESH_ACTION) {
            retained.push(command.clone());
        }
    }
    if retained.is_empty() {
        keys.remove("command");
    } else {
        keys["command"] = Item::ArrayOfTables(retained);
    }
    if keys.is_empty() {
        document.remove("keys");
    }
}

fn ensure_table<'a>(document: &'a mut DocumentMut, path: &[&str]) -> Result<&'a mut Table> {
    let mut item: &mut Item = document.as_item_mut();
    for key in path {
        let table = item
            .as_table_mut()
            .context("Herdr config section is not a table")?;
        item = table.entry(key).or_insert(Item::Table(Table::new()));
    }
    item.as_table_mut()
        .context("Herdr config section is not a table")
}

fn strip_quota_tokens(row: &Value) -> Array {
    let mut cleaned = Array::new();
    if let Some(items) = row.as_array() {
        for item in items {
            let is_quota_token =
                configured_token_name(item).is_some_and(|value| QUOTA_ROW_MARKERS.contains(&value));
            if !is_quota_token {
                cleaned.push(item.clone());
            }
        }
    }
    cleaned
}

fn configured_token_name(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| {
        value
            .as_inline_table()
            .and_then(|table| table.get("token"))
            .and_then(Value::as_str)
    })
}

fn add_provider_rows(agents: &mut Table, rows: &Array) -> Result<()> {
    let rows_by_agent = agents
        .entry("rows_by_agent")
        .or_insert(Item::Table(Table::new()))
        .as_table_mut()
        .context("Herdr ui.sidebar.agents.rows_by_agent must be a table")?;

    for (provider, color) in PROVIDER_STYLES {
        let is_managed = rows_by_agent
            .get(provider)
            .and_then(Item::as_value)
            .is_some_and(has_provider_style_marker);
        if rows_by_agent.contains_key(provider) && !is_managed {
            continue;
        }
        let mut value = Value::Array(provider_rows(rows, color));
        value
            .decor_mut()
            .set_suffix(format!(" # {PROVIDER_STYLE_MARKER}"));
        rows_by_agent.insert(provider, Item::Value(value));
    }
    Ok(())
}

fn provider_rows(rows: &Array, color: Option<&str>) -> Array {
    let mut themed = Array::new();
    for row in rows.iter() {
        let Some(items) = row.as_array() else {
            continue;
        };
        let mut themed_row = Array::new();
        for item in items {
            if configured_token_name(item) == Some("$quota_provider") {
                themed_row.push(styled_token("agent", color, Some(true), Some(false)));
            } else {
                themed_row.push(item.clone());
            }
        }
        themed.push(Value::Array(themed_row));
    }
    themed
}

fn remove_managed_provider_rows(agents: &mut Table) {
    let Some(rows_by_agent) = agents.get_mut("rows_by_agent").and_then(Item::as_table_mut) else {
        return;
    };
    for (provider, _) in PROVIDER_STYLES {
        let is_managed = rows_by_agent
            .get(provider)
            .and_then(Item::as_value)
            .is_some_and(has_provider_style_marker);
        if is_managed {
            rows_by_agent.remove(provider);
        }
    }
    if rows_by_agent.is_empty() {
        agents.remove("rows_by_agent");
    }
}

fn has_provider_style_marker(value: &Value) -> bool {
    value
        .decor()
        .suffix()
        .and_then(|suffix| suffix.as_str())
        .is_some_and(|suffix| suffix.contains(PROVIDER_STYLE_MARKER))
}

fn default_state_row() -> Array {
    let mut row = Array::new();
    row.push("state_icon");
    row.push("tab");
    row
}

fn normalize_official_row(row: Array) -> Array {
    let has_state_icon = row.iter().any(|item| item.as_str() == Some("state_icon"));
    if !has_state_icon || row.iter().any(|item| item.as_str() == Some("agent")) {
        return row;
    }
    let mut normalized = Array::new();
    let mut has_tab = false;
    for item in row {
        match item.as_str() {
            Some("workspace") | Some("pane") => {
                if !has_tab {
                    normalized.push("tab");
                    has_tab = true;
                }
            }
            Some("tab") => {
                if !has_tab {
                    has_tab = true;
                    normalized.push(item);
                }
            }
            Some("terminal_title_stripped") => {}
            _ => normalized.push(item),
        }
    }
    if !has_tab {
        let insert_at = normalized
            .iter()
            .position(|item| item.as_str() == Some("terminal_title_stripped"))
            .unwrap_or(normalized.len());
        normalized.insert(insert_at, "tab");
    }
    normalized
}

fn append_quota_rows(rows: &mut Array) {
    // Keep the official state row, but split each quota window onto its own
    // row. Herdr elides rows whose custom token is absent, so weekly-only
    // providers do not gain a blank five-hour row.
    for row in rows.iter_mut() {
        let Some(items) = row.as_array_mut() else {
            continue;
        };
        let has_state_icon = items.iter().any(|item| item.as_str() == Some("state_icon"));
        let mut cleaned = Array::new();
        for item in items.iter() {
            match item.as_str() {
                Some("terminal_title_stripped") | Some("$quota_topic") => {}
                Some("agent") if !has_state_icon => {}
                _ => cleaned.push(item.clone()),
            }
        }
        *items = cleaned;
    }

    let mut compacted_rows = Array::new();
    for row in rows.iter() {
        if row.as_array().is_some_and(|items| !items.is_empty()) {
            compacted_rows.push(row.clone());
        }
    }
    *rows = compacted_rows;

    let official_index = rows.iter().position(|row| {
        row.as_array().is_some_and(|items| {
            items.iter().any(|item| item.as_str() == Some("state_icon"))
                && !items.iter().any(|item| item.as_str() == Some("agent"))
        })
    });

    if let Some(index) = official_index {
        if let Some(row) = rows.get_mut(index).and_then(Value::as_array_mut) {
            row.push(styled_token(
                "$quota_provider",
                None,
                Some(true),
                Some(false),
            ));
        }
    }

    rows.push(Value::Array(styled_row(
        "$quota_topic",
        None,
        None,
        Some(false),
    )));

    append_window_rows(rows, "quota_5h");
    append_window_rows(rows, "quota_week");
}

fn append_window_rows(rows: &mut Array, base: &str) {
    rows.push(Value::Array(styled_row(
        &format!("${base}_normal"),
        Some(QUOTA_SAFE_COLOR),
        Some(true),
        Some(false),
    )));
    rows.push(Value::Array(styled_row(
        &format!("${base}_warning"),
        Some(QUOTA_WARNING_COLOR),
        Some(true),
        Some(false),
    )));
    rows.push(Value::Array(styled_row(
        &format!("${base}_danger"),
        Some(QUOTA_DANGER_COLOR),
        Some(true),
        Some(false),
    )));
}

fn styled_row(token: &str, fg: Option<&str>, bold: Option<bool>, dim: Option<bool>) -> Array {
    let mut row = Array::new();
    row.push(styled_token(token, fg, bold, dim));
    row
}

fn styled_token(token: &str, fg: Option<&str>, bold: Option<bool>, dim: Option<bool>) -> Value {
    let mut value = InlineTable::new();
    value.insert("token", Value::from(token));
    if let Some(fg) = fg {
        value.insert("fg", Value::from(fg));
    }
    if let Some(bold) = bold {
        value.insert("bold", Value::from(bold));
    }
    if let Some(dim) = dim {
        value.insert("dim", Value::from(dim));
    }
    Value::InlineTable(value)
}

fn print_diff_hint() {
    println!("  keep Herdr's official state icon and plane tab");
    println!("  show the user prompt before separate, severity-colored 5h/week rows");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_quota_rows_without_replacing_official_rows() {
        let original = r#"[ui.sidebar.agents]
rows = [["state_icon", "agent"]]
"#;
        let updated = add_quota_row(original).unwrap();
        assert!(updated.contains("$quota_5h"));
        assert!(updated.contains("$quota_week"));
        assert!(updated.contains("state_icon"));
        assert!(updated.contains("agent"));
        assert!(updated.contains("$quota_topic"));
        assert!(updated.contains("$quota_5h_warning"));
        assert!(updated.contains("$quota_week_danger"));
        assert_eq!(add_quota_row(&updated).unwrap(), updated);
    }

    #[test]
    fn removes_plugin_tokens_but_keeps_the_official_agent_row() {
        let original = r#"[ui.sidebar.agents]
rows = [["state_icon", "pane", "terminal_title_stripped"], ["agent", "$quota_icon", "$quota_5h"], ["$quota_week"]]
"#;
        let updated = remove_quota_row(original).unwrap();
        assert!(updated.contains("state_icon"));
        assert!(updated.contains("agent"));
        assert!(!updated.contains("$quota_summary"));
        assert!(!updated.contains("$quota_icon"));
        assert!(updated.contains("terminal_title_stripped"));
    }

    #[test]
    fn migrates_old_quota_only_rows_and_restores_herdr_state_row() {
        let original = r#"[ui.sidebar.agents]
rows = [["$quota_provider", "$quota_status"], ["$quota_summary"]]
"#;
        let updated = add_quota_row(original).unwrap();
        assert!(updated.contains("state_icon"));
        assert!(updated.contains("$quota_provider"));
        assert!(updated.contains("$quota_5h"));
        assert!(updated.contains("$quota_week"));
        assert_eq!(add_quota_row(&updated).unwrap(), updated);
    }

    #[test]
    fn preserves_user_owned_provider_rows() {
        let original = r#"[ui.sidebar.agents]
rows = [["state_icon", "agent"]]

[ui.sidebar.agents.rows_by_agent]
claude = [["state_icon", "agent"]]
"#;
        let updated = add_quota_row(original).unwrap();
        assert!(updated.contains("claude = [[\"state_icon\", \"agent\"]]"));
        assert!(updated.contains("codex ="));
        let removed = remove_quota_row(&updated).unwrap();
        assert!(removed.contains("claude = [[\"state_icon\", \"agent\"]]"));
        assert!(!removed.contains("codex ="));
    }
}
