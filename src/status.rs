use crate::cache::CacheStore;
use crate::model::{format_percent, Provider, ProviderSnapshot, WindowKind};
use crate::presentation::{balance_summary, format_reset_eta};
use anyhow::Result;

const SEPARATOR: &str = " · ";

/// Print a single-line quota strip for Herdr's tab bar.
///
/// The strip is polled non-interactively by Herdr's server. Every failure
/// degrades to empty stdout with exit code 0: a missing or corrupt cache
/// clears the tab-bar entry instead of surfacing an error.
pub fn run() -> Result<()> {
    // Deliberate exception to this command being cache-only: the OpenCode Go
    // collector has no event hook (it is excluded from the opencode pane
    // mapping), so this ~30s tab-bar poll is its only refresh trigger. The
    // refresh is per-provider debounced (60s), swallows every error, and never
    // publishes to panes; the line below still renders whatever the cache holds.
    let _ = crate::refresh::debounced_refresh(Provider::OpenCodeGo);
    let _ = crate::refresh::debounced_refresh(Provider::DeepSeek);
    let line = CacheStore::from_env()
        .and_then(|cache| status_line(&cache, CacheStore::now_unix()))
        .unwrap_or_default();
    if !line.is_empty() {
        println!("{line}");
    }
    Ok(())
}

fn status_line(cache: &CacheStore, now_unix: u64) -> Result<String> {
    let mut segments = Vec::new();
    for provider in Provider::ALL {
        if let Some(snapshot) = cache.load(provider)? {
            let group = match provider {
                Provider::OpenCodeGo => open_code_go_segment(&snapshot, now_unix),
                Provider::DeepSeek => deepseek_segment(&snapshot, now_unix),
                Provider::Codex => codex_segment(&snapshot, now_unix),
                _ => segment(&snapshot, now_unix),
            };
            if let Some(segment) = group {
                segments.push(segment);
            }
        }
    }
    Ok(segments.join(SEPARATOR))
}

/// Render a raw DeepSeek balance with a compact label and health symbol.
/// The status strip shares one line with every other provider, so DeepSeek uses
/// the short "DSk" marker instead of its full display name to avoid clipping.
fn deepseek_segment(snapshot: &ProviderSnapshot, now_unix: u64) -> Option<String> {
    let balance = balance_summary(snapshot)?;
    Some(format!("DSk {} {}", balance, snapshot.severity(now_unix).symbol()))
}

/// One `<kind> <percent>% reset <eta>` entry for a provider snapshot, or
/// nothing when the snapshot carries no quota windows at all.
fn segment(snapshot: &ProviderSnapshot, now_unix: u64) -> Option<String> {
    let window = snapshot.most_consumed_window()?;
    let label = format!(
        "{} {}%",
        snapshot.provider.agent_kind(),
        format_percent(window.remaining_percent)
    );
    let segment = match window.resets_at {
        Some(reset) => format!("{label} reset {}", format_reset_eta(reset, now_unix)),
        None => label,
    };
    Some(segment)
}

/// The Codex strip segment tracks the rolling ~5h window — the short-term
/// limit — falling back to the most-consumed window for caches that predate
/// the 5h window. The weekly window remains dashboard-popup detail.
fn codex_segment(snapshot: &ProviderSnapshot, now_unix: u64) -> Option<String> {
    let window = match snapshot.window(WindowKind::FiveHour) {
        Some(window) => window,
        None => snapshot.most_consumed_window()?,
    };
    let label = format!(
        "{} {}%",
        snapshot.provider.agent_kind(),
        format_percent(window.remaining_percent)
    );
    let segment = match window.resets_at {
        Some(reset) => format!("{label} reset {}", format_reset_eta(reset, now_unix)),
        None => label,
    };
    Some(segment)
}

/// The Go strip segment tracks the rolling ~5h window only: it is the
/// short-term limit worth glancing at while the tab bar has limited width.
/// The full rolling/weekly/monthly breakdown lives in the dashboard popup.
fn open_code_go_segment(snapshot: &ProviderSnapshot, now_unix: u64) -> Option<String> {
    let window = snapshot.window(WindowKind::FiveHour)?;
    let label = format!("go 5h {}%", format_percent(window.remaining_percent));
    let segment = match window.resets_at {
        Some(reset) => format!("{label} reset {}", format_reset_eta(reset, now_unix)),
        None => label,
    };
    Some(segment)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountBalance, ResetAt, UsageWindow, WindowKind};
    use tempfile::tempdir;

    fn weekly_snapshot(provider: Provider, used_percent: f64, reset_unix: u64) -> ProviderSnapshot {
        ProviderSnapshot::new(
            provider,
            vec![UsageWindow::new(
                WindowKind::Weekly,
                used_percent,
                Some(ResetAt::from_unix_seconds(reset_unix)),
            )
            .unwrap()],
            1,
        )
    }

    #[test]
    fn joins_present_providers_with_middle_dot_separator() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        cache
            .save(&weekly_snapshot(Provider::Codex, 11.4, 584_000))
            .unwrap();
        cache
            .save(&weekly_snapshot(Provider::Grok, 4.2, 156_000))
            .unwrap();

        let line = status_line(&cache, 0).unwrap();

        assert_eq!(line, "codex 89% reset 6d18h · grok 96% reset 1d19h");
    }

    #[test]
    fn single_provider_line_has_no_separator() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        cache
            .save(&weekly_snapshot(Provider::Grok, 4.2, 156_000))
            .unwrap();

        let line = status_line(&cache, 0).unwrap();

        assert_eq!(line, "grok 96% reset 1d19h");
    }

    #[test]
    fn empty_cache_renders_an_empty_line() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());

        let line = status_line(&cache, 0).unwrap();

        assert_eq!(line, "");
    }

    #[test]
    fn multi_window_provider_shows_the_most_consumed_window() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![
                UsageWindow::new(
                    WindowKind::FiveHour,
                    20.0,
                    Some(ResetAt::from_unix_seconds(156_000)),
                )
                .unwrap(),
                UsageWindow::new(
                    WindowKind::Weekly,
                    90.0,
                    Some(ResetAt::from_unix_seconds(584_000)),
                )
                .unwrap(),
            ],
            1,
        );

        assert_eq!(
            segment(&snapshot, 0).as_deref(),
            Some("claude 10% reset 6d18h")
        );
    }

    /// The strip tracks Codex's rolling ~5h window; the weekly window is
    /// dashboard-popup detail.
    #[test]
    fn codex_strip_prefers_the_five_hour_window() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let snapshot = ProviderSnapshot::new(
            Provider::Codex,
            vec![
                UsageWindow::new(
                    WindowKind::FiveHour,
                    20.0,
                    Some(ResetAt::from_unix_seconds(156_000)),
                )
                .unwrap(),
                UsageWindow::new(
                    WindowKind::Weekly,
                    61.0,
                    Some(ResetAt::from_unix_seconds(584_000)),
                )
                .unwrap(),
            ],
            1,
        );
        cache.save(&snapshot).unwrap();

        let line = status_line(&cache, 0).unwrap();

        assert_eq!(line, "codex 80% reset 1d19h");
    }

    #[test]
    fn codex_strip_falls_back_without_a_five_hour_window() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        cache
            .save(&weekly_snapshot(Provider::Codex, 61.0, 584_000))
            .unwrap();

        let line = status_line(&cache, 0).unwrap();

        assert_eq!(line, "codex 39% reset 6d18h");
    }

    /// Mirrors the live endpoint contract: rolling 0% used, weekly 0% used,
    /// monthly 72% used. The strip renders only the rolling ~5h window; the
    /// weekly and monthly windows are dashboard-popup detail.
    #[test]
    fn go_strip_shows_only_the_five_hour_window() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        let snapshot = ProviderSnapshot::new(
            Provider::OpenCodeGo,
            vec![
                UsageWindow::new(
                    WindowKind::FiveHour,
                    0.0,
                    Some(ResetAt::from_unix_seconds(35_465)),
                )
                .unwrap(),
                UsageWindow::new(
                    WindowKind::Weekly,
                    0.0,
                    Some(ResetAt::from_unix_seconds(555_600)),
                )
                .unwrap(),
                UsageWindow::new(
                    WindowKind::Monthly,
                    72.0,
                    Some(ResetAt::from_unix_seconds(382_135)),
                )
                .unwrap(),
            ],
            1,
        );
        cache.save(&snapshot).unwrap();

        let line = status_line(&cache, 0).unwrap();

        assert_eq!(line, "go 5h 100% reset 9h51m");
    }

    #[test]
    fn deepseek_status_uses_raw_balance_and_health_symbol() {
        let snapshot = ProviderSnapshot::new(Provider::DeepSeek, vec![], 1).with_balance(Some(
            AccountBalance {
                currency: "USD".to_string(),
                total_balance: "4.07".to_string(),
                is_available: true,
            },
        ));
        assert_eq!(
            deepseek_segment(&snapshot, 0).as_deref(),
            Some("DSk 4.07 USD ●")
        );
    }

    #[test]
    fn go_segment_skipped_without_a_five_hour_window_and_skips_unknown_reset() {
        let directory = tempdir().unwrap();
        let cache = CacheStore::new(directory.path());
        cache
            .save(&ProviderSnapshot::new(
                Provider::OpenCodeGo,
                vec![UsageWindow::new(WindowKind::Monthly, 100.0, None).unwrap()],
                1,
            ))
            .unwrap();
        assert_eq!(status_line(&cache, 0).unwrap(), "");

        cache
            .save(&ProviderSnapshot::new(
                Provider::OpenCodeGo,
                vec![UsageWindow::new(WindowKind::FiveHour, 0.0, None).unwrap()],
                2,
            ))
            .unwrap();
        assert_eq!(status_line(&cache, 0).unwrap(), "go 5h 100%");
    }
}
