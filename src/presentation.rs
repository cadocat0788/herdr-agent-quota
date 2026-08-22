use crate::model::{
    format_percent, Provider, ProviderSnapshot, ResetAt, Severity, UsageWindow, WindowKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataTokens {
    pub quota_badge: String,
    pub quota_state: String,
    pub quota_icon: String,
    pub quota_provider: String,
    pub quota_status: String,
    pub quota_5h: String,
    pub quota_5h_severity: Option<Severity>,
    pub quota_week: String,
    pub quota_week_severity: Option<Severity>,
    pub quota_summary: String,
    pub quota_error: Option<String>,
    // Overall snapshot severity, so a pane fed by several providers can pick
    // which one owns its shared identity slots.
    pub severity: Severity,
}

impl MetadataTokens {
    pub fn from_snapshot(snapshot: &ProviderSnapshot, now_unix: u64) -> Self {
        let severity = snapshot.severity(now_unix);
        Self {
            quota_badge: snapshot.provider.badge().to_string(),
            quota_state: severity.symbol().to_string(),
            quota_icon: snapshot.provider.icon().to_string(),
            quota_provider: snapshot.provider.display_name().to_string(),
            quota_status: severity.label().to_string(),
            quota_5h: sidebar_window(snapshot, WindowKind::FiveHour, now_unix),
            quota_5h_severity: window_severity(snapshot, WindowKind::FiveHour, now_unix),
            quota_week: sidebar_window(snapshot, WindowKind::Weekly, now_unix),
            quota_week_severity: window_severity(snapshot, WindowKind::Weekly, now_unix),
            quota_summary: sidebar_summary(snapshot, now_unix),
            quota_error: None,
            severity,
        }
    }

    pub fn unavailable(provider: Provider, reason: impl Into<String>) -> Self {
        Self {
            quota_badge: provider.badge().to_string(),
            quota_state: Severity::Unknown.symbol().to_string(),
            quota_icon: provider.icon().to_string(),
            quota_provider: provider.display_name().to_string(),
            quota_status: Severity::Unknown.label().to_string(),
            quota_5h: match provider {
                Provider::Claude | Provider::Agy => "5h N/A".to_string(),
                Provider::Codex | Provider::Grok => String::new(),
            },
            quota_5h_severity: match provider {
                Provider::Claude | Provider::Agy => Some(Severity::Unknown),
                Provider::Codex | Provider::Grok => None,
            },
            quota_week: "week N/A".to_string(),
            quota_week_severity: Some(Severity::Unknown),
            quota_summary: "unavailable".to_string(),
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

fn summary(snapshot: &ProviderSnapshot, now_unix: u64, include_left: bool) -> String {
    [WindowKind::FiveHour, WindowKind::Weekly]
        .into_iter()
        .filter_map(|kind| snapshot.window(kind))
        .map(|window| format_window(window, now_unix, include_left))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn sidebar_window(snapshot: &ProviderSnapshot, kind: WindowKind, now_unix: u64) -> String {
    snapshot
        .window(kind)
        .map(|window| format_window(window, now_unix, false))
        .unwrap_or_default()
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

fn format_reset_eta(reset_at: ResetAt, now_unix: u64) -> String {
    let seconds = reset_at.unix_seconds().saturating_sub(now_unix);
    if seconds == 0 {
        return "due".to_string();
    }
    let minutes = (seconds / 60).max(1);
    if minutes >= 24 * 60 {
        return format!("{}d{}h", minutes / (24 * 60), (minutes % (24 * 60)) / 60);
    }
    if minutes >= 60 {
        return format!("{}h{:02}m", minutes / 60, minutes % 60);
    }
    format!("{minutes}m")
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
}
