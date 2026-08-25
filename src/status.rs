use crate::cache::CacheStore;
use crate::model::{format_percent, Provider, ProviderSnapshot, UsageWindow};
use crate::presentation::format_reset_eta;
use anyhow::Result;

const SEPARATOR: &str = " · ";

/// Print a single-line quota strip for Herdr's tab bar.
///
/// The strip is polled non-interactively by Herdr's server, so it never
/// touches raw mode and never reads anything but local cache files. Every
/// failure degrades to empty stdout with exit code 0: a missing or corrupt
/// cache clears the tab-bar entry instead of surfacing an error.
pub fn run() -> Result<()> {
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
            if let Some(segment) = segment(&snapshot, now_unix) {
                segments.push(segment);
            }
        }
    }
    Ok(segments.join(SEPARATOR))
}

/// One `<kind> <percent>% reset <eta>` entry for a provider snapshot, or
/// nothing when the snapshot carries no quota windows at all.
fn segment(snapshot: &ProviderSnapshot, now_unix: u64) -> Option<String> {
    let window = most_consumed_window(&snapshot.windows)?;
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

/// Pick the window closest to exhaustion, so a multi-window provider shows
/// whichever limit will bite first.
fn most_consumed_window(windows: &[UsageWindow]) -> Option<&UsageWindow> {
    windows.iter().min_by(|left, right| {
        left.remaining_percent
            .partial_cmp(&right.remaining_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ResetAt, WindowKind};
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
}
