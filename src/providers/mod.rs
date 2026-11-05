pub mod agy;
pub mod claude;
pub mod codex;
pub mod deepseek;
pub mod grok;
pub mod opencode_go;
pub mod statusline;
pub mod zcode_glm;

use thiserror::Error;
use std::path::PathBuf;

#[derive(Debug, PartialEq, Eq, Error)]
pub enum ProviderError {
    #[error("provider credentials are unavailable")]
    MissingCredentials,
    #[error("provider quota is unavailable: {0}")]
    Unavailable(String),
    #[error("provider response is not supported: {0}")]
    UnsupportedResponse(String),
    #[error("provider request failed: {0}")]
    Request(String),
}

/// Auth-store candidates in precedence order: the caller's own override, Pi's
/// configured agent dir, Pi's default home, then Gentle Shell's isolated home —
/// the store Pi actually writes when it runs under Gentle Shell.
pub fn auth_store_candidates(override_var: Option<&str>) -> Vec<PathBuf> {
    auth_store_candidates_from(override_var, &|name| std::env::var_os(name))
}

fn auth_store_candidates_from(
    override_var: Option<&str>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(name) = override_var {
        if let Some(value) = env(name) {
            if !value.is_empty() {
                paths.push(PathBuf::from(value));
            }
        }
    }
    if let Some(directory) = env("PI_CODING_AGENT_DIR") {
        if !directory.is_empty() {
            paths.push(PathBuf::from(directory).join("auth.json"));
        }
    }
    if let Some(home) = env("HOME") {
        let home = PathBuf::from(home);
        paths.push(home.join(".pi").join("agent").join("auth.json"));
        paths.push(home.join(".gentle-shell").join("agent").join("auth.json"));
    }
    paths
}

/// The first store in the list whose JSON yields an entry for this provider, so
/// a store that exists but lacks the entry does not hide a later one. Missing
/// and unparsable files fall through silently; the provider reports the absence
/// itself, keeping the collector fail-silent.
pub fn find_in_store_list<T>(
    paths: &[PathBuf],
    extract: impl Fn(&serde_json::Value) -> Option<T>,
) -> Option<(PathBuf, T)> {
    for path in paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if let Some(found) = extract(&value) {
            return Some((path.clone(), found));
        }
    }
    None
}

pub fn find_in_auth_stores<T>(
    override_var: Option<&str>,
    extract: impl Fn(&serde_json::Value) -> Option<T>,
) -> Option<(PathBuf, T)> {
    find_in_store_list(&auth_store_candidates(override_var), extract)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, OsString)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect();
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn auth_candidates_prefer_the_override_then_pi_then_gentle_shell() {
        let env = env_from(&[
            ("ZCODE_GLM_AUTH_FILE", "/tmp/override.json"),
            ("PI_CODING_AGENT_DIR", "/tmp/agent"),
            ("HOME", "/home/tester"),
        ]);
        assert_eq!(
            auth_store_candidates_from(Some("ZCODE_GLM_AUTH_FILE"), &env),
            vec![
                PathBuf::from("/tmp/override.json"),
                PathBuf::from("/tmp/agent/auth.json"),
                PathBuf::from("/home/tester/.pi/agent/auth.json"),
                PathBuf::from("/home/tester/.gentle-shell/agent/auth.json"),
            ]
        );
    }

    #[test]
    fn auth_candidates_skip_unset_and_empty_variables() {
        let env = env_from(&[("PI_CODING_AGENT_DIR", ""), ("HOME", "/home/tester")]);
        assert_eq!(
            auth_store_candidates_from(None, &env),
            vec![
                PathBuf::from("/home/tester/.pi/agent/auth.json"),
                PathBuf::from("/home/tester/.gentle-shell/agent/auth.json"),
            ]
        );
    }

    #[test]
    fn a_store_without_the_entry_falls_through_to_the_next() {
        let dir = std::env::temp_dir().join(format!("quota-auth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.json");
        let second = dir.join("second.json");
        std::fs::write(&first, r#"{"openai-codex":{"key":"a"}}"#).unwrap();
        std::fs::write(&second, r#"{"deepseek":{"key":"sk-test"}}"#).unwrap();
        let found = find_in_store_list(&[folder(&first), folder(&second)], |value| {
            value.get("deepseek")?.get("key")?.as_str().map(str::to_string)
        });
        assert_eq!(found, Some((second.clone(), "sk-test".to_string())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_store_file_is_skipped() {
        let missing = std::env::temp_dir().join("quota-auth-does-not-exist.json");
        std::fs::remove_file(&missing).ok();
        assert!(find_in_store_list(&[missing], |_| Some(())).is_none());
    }

    fn folder(path: &std::path::Path) -> PathBuf {
        path.to_path_buf()
    }
}
