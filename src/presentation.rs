use crate::model::{
    format_percent, Provider, ProviderSnapshot, ResetAt, Severity, UsageWindow, WindowKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataTokens {
    pub quota_state: String,
    pub quota_icon: String,
    pub quota_provider: String,
    pub quota_status: String,
    pub quota_5h: String,
    pub quota_5h_severity: Option<Severity>,
    pub quota_week: String,
    pub quota_week_severity: Option<Severity>,
    pub quota_summary: String,
    pub quota_context: String,
    pub quota_cache: String,
    pub quota_cache_ttl: String,
    pub quota_error: Option<String>,
    // Overall snapshot severity, so a pane fed by several providers can pick
    // which one owns its shared identity slots.
    pub severity: Severity,
}

impl MetadataTokens {
    pub fn from_snapshot(snapshot: &ProviderSnapshot, now_unix: u64) -> Self {
        let severity = snapshot.severity(now_unix);
        Self {
            quota_state: severity.symbol().to_string(),
            quota_icon: snapshot.provider.icon().to_string(),
            quota_provider: snapshot.provider.display_name().to_string(),
            quota_status: severity.label().to_string(),
            quota_5h: sidebar_window(snapshot, WindowKind::FiveHour, now_unix),
            quota_5h_severity: window_severity(snapshot, WindowKind::FiveHour, now_unix),
            quota_week: sidebar_window(snapshot, WindowKind::Weekly, now_unix),
            quota_week_severity: window_severity(snapshot, WindowKind::Weekly, now_unix),
            quota_summary: sidebar_summary(snapshot, now_unix),
            quota_context: sidebar_context(snapshot),
            quota_cache: sidebar_cache(snapshot),
            quota_cache_ttl: sidebar_cache_ttl(snapshot, now_unix),
            quota_error: sidebar_cache_error(snapshot, now_unix),
            severity,
        }
    }

    pub fn unavailable(provider: Provider, reason: impl Into<String>) -> Self {
        Self {
            quota_state: Severity::Unknown.symbol().to_string(),
            quota_icon: provider.icon().to_string(),
            quota_provider: provider.display_name().to_string(),
            quota_status: Severity::Unknown.label().to_string(),
            quota_5h: match provider {
                Provider::Claude | Provider::Agy | Provider::OpenCodeGo | Provider::ZcodeGlm => "5h N/A".to_string(),
                Provider::Codex | Provider::Grok | Provider::DeepSeek => String::new(),
            },
            quota_5h_severity: match provider {
                Provider::Claude | Provider::Agy | Provider::OpenCodeGo | Provider::ZcodeGlm => Some(Severity::Unknown),
                Provider::Codex | Provider::Grok | Provider::DeepSeek => None,
            },
            quota_week: match provider {
                Provider::DeepSeek => String::new(),
                _ => "7d N/A".to_string(),
            },
            quota_week_severity: match provider {
                Provider::DeepSeek => None,
                _ => Some(Severity::Unknown),
            },
            quota_summary: "unavailable".to_string(),
            quota_context: String::new(),
            quota_cache: String::new(),
            quota_cache_ttl: String::new(),
            quota_error: Some(reason.into().chars().take(80).collect()),
            severity: Severity::Unknown,
        }
    }
}

fn window_severity(
    snapshot: &ProviderSnapshot,
    kind: WindowKind,
    now_unix: u64,
) -> Option<Severity> {
    snapshot
        .window(kind)
        .map(|window| Severity::for_window(window, now_unix))
}

pub fn sidebar_summary(snapshot: &ProviderSnapshot, now_unix: u64) -> String {
    summary(snapshot, now_unix, false)
}

pub fn dashboard_summary(snapshot: &ProviderSnapshot, now_unix: u64) -> String {
    summary(snapshot, now_unix, true)
}

pub fn balance_summary(snapshot: &ProviderSnapshot) -> Option<String> {
    snapshot
        .balance
        .as_ref()
        .map(|balance| format!("{} {}", balance.total_balance, balance.currency))
}

fn summary(snapshot: &ProviderSnapshot, now_unix: u64, include_left: bool) -> String {
    if let Some(balance) = balance_summary(snapshot) {
        return balance;
    }
    [
        WindowKind::FiveHour,
        WindowKind::Weekly,
        WindowKind::Monthly,
    ]
    .into_iter()
    .filter_map(|kind| snapshot.window(kind))
    .map(|window| format_window(window, now_unix, include_left))
    .collect::<Vec<_>>()
    .join(" · ")
}

fn sidebar_window(snapshot: &ProviderSnapshot, kind: WindowKind, now_unix: u64) -> String {
    snapshot
        .window(kind)
        .map(|window| {
            if kind == WindowKind::Weekly && snapshot.window(WindowKind::FiveHour).is_none() {
                format_window(window, now_unix, false)
            } else {
                format_compact_window(window, now_unix)
            }
        })
        .unwrap_or_default()
}

fn sidebar_context(snapshot: &ProviderSnapshot) -> String {
    let Some(context) = snapshot.context.as_ref() else {
        return String::new();
    };
    format!("context {}%", format_percent(context.used_percent))
}

fn sidebar_cache(snapshot: &ProviderSnapshot) -> String {
    let Some(totals) = snapshot
        .context
        .as_ref()
        .and_then(|context| context.cache.as_ref())
        .and_then(|cache| cache.session_totals.as_ref())
    else {
        return String::new();
    };
    format!("cache {:.1}%", totals.hit_percent)
}

fn sidebar_cache_ttl(snapshot: &ProviderSnapshot, now_unix: u64) -> String {
    let Some(cache) = snapshot
        .context
        .as_ref()
        .and_then(|context| context.cache.as_ref())
    else {
        return String::new();
    };
    cache
        .remaining_ttl_seconds(now_unix)
        .filter(|remaining| *remaining > 0)
        .map(|remaining| format!("ttl≈{}", format_ttl(remaining)))
        .unwrap_or_default()
}

fn sidebar_cache_error(snapshot: &ProviderSnapshot, now_unix: u64) -> Option<String> {
    snapshot
        .context
        .as_ref()
        .and_then(|context| context.cache.as_ref())
        .and_then(|cache| cache.remaining_ttl_seconds(now_unix))
        .filter(|remaining| *remaining == 0)
        .map(|_| "no cached".to_string())
}

fn format_window(window: &UsageWindow, now_unix: u64, include_left: bool) -> String {
    let percent = format!("{}%", format_percent(window.remaining_percent));
    let left = if include_left { " left" } else { "" };
    let label = format!("{} {percent}{left}", window.kind.label());
    let Some(reset) = window.resets_at else {
        return label;
    };
    let eta = format_reset_eta(reset, now_unix);
    format!("{label} reset {eta}")
}

fn format_compact_window(window: &UsageWindow, now_unix: u64) -> String {
    let label = match window.kind {
        WindowKind::FiveHour => "5h",
        WindowKind::Weekly => "7d",
        WindowKind::Monthly => "month",
    };
    let percent = format!("{}%", format_percent(window.remaining_percent));
    let Some(reset) = window.resets_at else {
        return format!("{label} {percent}");
    };
    format!("{label} {percent} {}", format_reset_eta(reset, now_unix))
}

pub fn format_reset_eta(reset_at: ResetAt, now_unix: u64) -> String {
    let seconds = reset_at.unix_seconds().saturating_sub(now_unix);
    if seconds == 0 {
        return "due".to_string();
    }
    format_duration(seconds)
}

fn format_duration(seconds: u64) -> String {
    let minutes = (seconds / 60).max(1);
    if minutes >= 24 * 60 {
        return format!("{}d{}h", minutes / (24 * 60), (minutes % (24 * 60)) / 60);
    }
    if minutes >= 60 {
        return format!("{}h{:02}m", minutes / 60, minutes % 60);
    }
    format!("{minutes}m")
}

fn format_ttl(seconds: u64) -> String {
    if seconds == 0 {
        return "0m".to_string();
    }
    let minutes = seconds / 60;
    if (60..24 * 60).contains(&minutes) && minutes.is_multiple_of(60) {
        return format!("{}h", minutes / 60);
    }
    format_duration(seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProviderSnapshot, UsageWindow};

    fn window(kind: WindowKind, used: f64, reset: u64) -> UsageWindow {
        UsageWindow::new(kind, used, Some(ResetAt::from_unix_seconds(reset))).unwrap()
    }

    #[test]
    fn formats_reset_eta_for_minutes_hours_days_and_due_windows() {
        assert_eq!(
            format_reset_eta(ResetAt::from_unix_seconds(2_700), 0),
            "45m"
        );
        assert_eq!(
            format_reset_eta(ResetAt::from_unix_seconds(14_820), 0),
            "4h07m"
        );
        assert_eq!(
            format_reset_eta(ResetAt::from_unix_seconds(183_600), 0),
            "2d3h"
        );
        assert_eq!(format_reset_eta(ResetAt::from_unix_seconds(99), 100), "due");
        assert_eq!(format_ttl(0), "0m");
        assert_eq!(format_ttl(3_600), "1h");
    }

    #[test]
    fn summary_is_window_driven_and_keeps_five_hour_before_weekly() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![
                window(WindowKind::Weekly, 27.0, 183_600),
                window(WindowKind::FiveHour, 58.0, 14_820),
            ],
            0,
        );
        assert_eq!(
            sidebar_summary(&snapshot, 0),
            "5h 42% reset 4h07m · week 73% reset 2d3h"
        );
        assert_eq!(
            dashboard_summary(&snapshot, 0),
            "5h 42% left reset 4h07m · week 73% left reset 2d3h"
        );
    }

    #[test]
    fn sidebar_windows_use_consistent_single_spacing() {
        let five_hour = format_window(&window(WindowKind::FiveHour, 57.0, 14_820), 0, false);
        let weekly = format_window(&window(WindowKind::Weekly, 75.0, 183_600), 0, false);
        assert_eq!(five_hour, "5h 43% reset 4h07m");
        assert_eq!(weekly, "week 25% reset 2d3h");
    }

    #[test]
    fn opencode_go_windows_render_including_the_month_label() {
        let snapshot = ProviderSnapshot::new(
            Provider::OpenCodeGo,
            vec![
                window(WindowKind::FiveHour, 0.0, 14_820),
                window(WindowKind::Weekly, 0.0, 183_600),
                window(WindowKind::Monthly, 72.0, 400_000),
            ],
            0,
        );
        assert_eq!(
            sidebar_summary(&snapshot, 0),
            "5h 100% reset 4h07m · week 100% reset 2d3h · month 28% reset 4d15h"
        );
        assert_eq!(
            dashboard_summary(&snapshot, 0),
            "5h 100% left reset 4h07m · week 100% left reset 2d3h · month 28% left reset 4d15h"
        );
    }

    #[test]
    fn metadata_error_stays_within_herdr_token_limit() {
        let values = MetadataTokens::unavailable(Provider::Grok, "x".repeat(120));
        assert_eq!(values.quota_error.as_deref().unwrap().len(), 80);
    }

    #[test]
    fn metadata_keeps_severity_per_quota_window() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![
                window(WindowKind::FiveHour, 70.0, 14_820),
                window(WindowKind::Weekly, 90.0, 183_600),
            ],
            0,
        );
        let values = MetadataTokens::from_snapshot(&snapshot, 0);
        assert_eq!(values.quota_5h_severity, Some(Severity::Warning));
        assert_eq!(values.quota_week_severity, Some(Severity::Danger));
    }

    #[test]
    fn metadata_uses_compact_quota_window_labels() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![
                window(WindowKind::FiveHour, 58.0, 14_820),
                window(WindowKind::Weekly, 27.0, 183_600),
            ],
            0,
        );
        let values = MetadataTokens::from_snapshot(&snapshot, 0);
        assert_eq!(values.quota_5h, "5h 42% 4h07m");
        assert_eq!(values.quota_week, "7d 73% 2d3h");
    }

    #[test]
    fn metadata_formats_context_usage_when_available() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![window(WindowKind::Weekly, 10.0, 183_600)],
            0,
        )
        .with_context(Some(crate::model::ContextUsage::new(23.5).unwrap()));
        let values = MetadataTokens::from_snapshot(&snapshot, 0);
        assert_eq!(values.quota_context, "context 24%");
    }

    #[test]
    fn metadata_formats_session_cache_hit_rate_and_approximate_ttl() {
        let cache = crate::model::CacheUsage::from_token_counts(100, 800, 100)
            .unwrap()
            .with_ttl_estimate(60 * 60, 0)
            .with_session_totals(
                crate::model::CacheTotals::from_token_counts(100, 800, 100),
                "session-1",
                1,
            );
        let context = crate::model::ContextUsage::new(23.5)
            .unwrap()
            .with_cache(Some(cache));
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![window(WindowKind::Weekly, 10.0, 183_600)],
            0,
        )
        .with_context(Some(context));
        let values = MetadataTokens::from_snapshot(&snapshot, 0);
        assert_eq!(values.quota_context, "context 24%");
        assert_eq!(values.quota_cache, "cache 80.0%");
        assert_eq!(values.quota_cache_ttl, "ttl≈1h");
        assert_eq!(values.quota_error, None);
    }

    #[test]
    fn expired_cache_ttl_is_reported_as_no_cached() {
        let cache = crate::model::CacheUsage::from_token_counts(100, 800, 100)
            .unwrap()
            .with_ttl_estimate(60, 0)
            .with_session_totals(
                crate::model::CacheTotals::from_token_counts(100, 800, 100),
                "session-1",
                1,
            );
        let snapshot = ProviderSnapshot::new(Provider::Claude, vec![], 0).with_context(Some(
            crate::model::ContextUsage::new(23.5)
                .unwrap()
                .with_cache(Some(cache)),
        ));
        let values = MetadataTokens::from_snapshot(&snapshot, 61);
        assert_eq!(values.quota_cache_ttl, "");
        assert_eq!(values.quota_error.as_deref(), Some("no cached"));
    }

    #[test]
    fn weekly_only_sidebar_window_keeps_the_readable_reset_label() {
        let snapshot = ProviderSnapshot::new(
            Provider::Codex,
            vec![window(WindowKind::Weekly, 31.0, 518_400)],
            0,
        );
        let values = MetadataTokens::from_snapshot(&snapshot, 0);
        assert_eq!(values.quota_week, "week 69% reset 6d0h");
    }

    #[test]
    fn session_cache_percentage_keeps_one_decimal_instead_of_rounding_to_100() {
        let cache = crate::model::CacheUsage::from_token_counts(2_000, 433_336, 1_655)
            .unwrap()
            .with_session_totals(
                crate::model::CacheTotals::from_token_counts(2_000, 433_336, 1_655),
                "session-1",
                1,
            );
        let snapshot = ProviderSnapshot::new(Provider::Claude, vec![], 0).with_context(Some(
            crate::model::ContextUsage::new(43.0)
                .unwrap()
                .with_cache(Some(cache)),
        ));
        let values = MetadataTokens::from_snapshot(&snapshot, 0);
        assert_eq!(values.quota_cache, "cache 99.2%");
    }
}
