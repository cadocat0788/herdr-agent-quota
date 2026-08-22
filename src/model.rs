use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Grok,
    Claude,
    Agy,
}

impl Provider {
    pub const ALL: [Self; 4] = [Self::Codex, Self::Grok, Self::Claude, Self::Agy];

    pub fn badge(self) -> &'static str {
        match self {
            Self::Codex => "[C]",
            Self::Grok => "[X]",
            Self::Claude => "[A]",
            Self::Agy => "[G]",
        }
    }

    /// Compact text marker for a narrow Herdr sidebar. Plugin v1 accepts text
    /// tokens rather than provider SVGs, so the letters keep it recognizable.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Codex => "◈C",
            Self::Grok => "✕G",
            Self::Claude => "✦Cl",
            Self::Agy => "△Ag",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Grok => "Grok",
            Self::Claude => "Claude",
            Self::Agy => "Agy",
        }
    }

    pub fn source(self) -> &'static str {
        match self {
            Self::Codex => "codex-app-server",
            Self::Grok => "grok-cli-billing",
            Self::Claude => "claude-statusline",
            Self::Agy => "agy-statusline",
        }
    }

    /// Lowercase Herdr agent-kind name for this data source, used to label
    /// window rows on cards fed by several providers.
    pub fn agent_kind(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Grok => "grok",
            Self::Claude => "claude",
            Self::Agy => "agy",
        }
    }
}

impl Provider {
    /// Single mapping from Herdr's agent kind to the data sources that feed
    /// one of its cards. Most kinds map to exactly one provider; an agent
    /// kind like `opencode` runs several agents, so its card is fed by every
    /// subscription those agents consume.
    pub fn providers_for_agent(kind: &str) -> Option<Vec<Self>> {
        match kind.trim().to_ascii_lowercase().as_str() {
            "codex" => Some(vec![Self::Codex]),
            "grok" => Some(vec![Self::Grok]),
            "claude" | "claude-code" | "anthropic" => Some(vec![Self::Claude]),
            "agy" | "antigravity" | "antigravity-cli" => Some(vec![Self::Agy]),
            "opencode" => Some(vec![Self::Codex, Self::Grok]),
            _ => None,
        }
    }
}

impl std::str::FromStr for Provider {
    type Err = ModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Only 1:1 kinds name a single provider identity; multi-provider
        // kinds (opencode) deliberately fail to parse as a lone Provider.
        match Self::providers_for_agent(value).as_deref() {
            Some([provider]) => Ok(*provider),
            _ => Err(ModelError::UnknownProvider(
                value.trim().to_ascii_lowercase(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    FiveHour,
    Weekly,
}

impl WindowKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5h",
            Self::Weekly => "week",
        }
    }

    pub fn duration_seconds(self) -> u64 {
        match self {
            Self::FiveHour => 5 * 60 * 60,
            Self::Weekly => 7 * 24 * 60 * 60,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ResetAt(u64);

impl ResetAt {
    pub fn from_unix_seconds(seconds: u64) -> Self {
        Self(seconds)
    }

    pub fn parse_rfc3339(value: &str) -> Option<Self> {
        let timestamp = OffsetDateTime::parse(value, &Rfc3339)
            .ok()?
            .unix_timestamp();
        u64::try_from(timestamp).ok().map(Self)
    }

    pub fn parse(value: &str) -> Option<Self> {
        value
            .parse::<u64>()
            .ok()
            .map(Self)
            .or_else(|| Self::parse_rfc3339(value))
    }

    pub fn after(base_unix: u64, seconds: u64) -> Self {
        Self(base_unix.saturating_add(seconds))
    }

    pub fn unix_seconds(self) -> u64 {
        self.0
    }
}

impl<'de> Deserialize<'de> for ResetAt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Unix(u64),
            Text(String),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Unix(value) => Ok(Self(value)),
            Repr::Text(value) => Self::parse(&value).ok_or_else(|| {
                serde::de::Error::custom("reset time is not Unix seconds or RFC 3339")
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub kind: WindowKind,
    pub used_percent: f64,
    pub remaining_percent: f64,
    pub resets_at: Option<ResetAt>,
}

impl UsageWindow {
    pub fn new(
        kind: WindowKind,
        used_percent: f64,
        resets_at: Option<ResetAt>,
    ) -> Result<Self, ModelError> {
        if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
            return Err(ModelError::InvalidPercentage(used_percent));
        }
        Ok(Self {
            kind,
            used_percent,
            remaining_percent: (100.0 - used_percent).clamp(0.0, 100.0),
            resets_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderSnapshot {
    pub provider: Provider,
    pub source: String,
    pub fetched_at_unix: u64,
    pub windows: Vec<UsageWindow>,
}

impl ProviderSnapshot {
    pub fn new(provider: Provider, windows: Vec<UsageWindow>, fetched_at_unix: u64) -> Self {
        Self {
            provider,
            source: provider.source().to_string(),
            fetched_at_unix,
            windows,
        }
    }

    pub fn window(&self, kind: WindowKind) -> Option<&UsageWindow> {
        self.windows.iter().find(|window| window.kind == kind)
    }

    pub fn severity(&self, now_unix: u64) -> Severity {
        let relevant = match self.provider {
            Provider::Codex | Provider::Grok => self.window(WindowKind::Weekly),
            Provider::Claude | Provider::Agy => self
                .window(WindowKind::FiveHour)
                .or_else(|| self.window(WindowKind::Weekly)),
        };
        relevant
            .map(|window| Severity::for_window(window, now_unix))
            .unwrap_or(Severity::Unknown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Danger,
    Unknown,
}

impl Severity {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Normal => "●",
            Self::Warning => "▲",
            Self::Danger => "!",
            Self::Unknown => "?",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "OK",
            Self::Warning => "WARN",
            Self::Danger => "LOW",
            Self::Unknown => "N/A",
        }
    }

    pub fn for_window(window: &UsageWindow, now_unix: u64) -> Self {
        let Some(reset_at) = window.resets_at else {
            return Self::Unknown;
        };
        let remaining_seconds = reset_at.unix_seconds().saturating_sub(now_unix);
        if remaining_seconds == 0 {
            return Self::Unknown;
        }

        let remaining_time_percent = remaining_seconds.min(window.kind.duration_seconds()) as f64
            / window.kind.duration_seconds() as f64
            * 100.0;
        if window.remaining_percent >= remaining_time_percent {
            Self::Normal
        } else if window.remaining_percent < 20.0 {
            Self::Danger
        } else {
            Self::Warning
        }
    }
}

pub fn format_percent(value: f64) -> String {
    format!("{value:.0}")
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("unknown provider: {0}")]
    UnknownProvider(String),
    #[error("percentage must be finite and between 0 and 100, got {0}")]
    InvalidPercentage(f64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(kind: WindowKind, used: f64) -> UsageWindow {
        UsageWindow::new(kind, used, None).expect("fixture percentage is valid")
    }

    #[test]
    fn remaining_percentage_is_derived_from_used_percentage() {
        let value = window(WindowKind::Weekly, 42.5);
        assert_eq!(value.remaining_percent, 57.5);
        assert_eq!(format_percent(value.remaining_percent), "58");
    }

    #[test]
    fn reset_time_deserializes_new_unix_and_legacy_rfc3339_cache_values() {
        let unix: ResetAt = serde_json::from_str("1787400000").unwrap();
        let legacy: ResetAt = serde_json::from_str("\"2026-08-22T12:00:00Z\"").unwrap();
        assert_eq!(unix, ResetAt::from_unix_seconds(1_787_400_000));
        assert_eq!(legacy, unix);
        assert_eq!(serde_json::to_string(&unix).unwrap(), "1787400000");
    }

    #[test]
    fn severity_compares_quota_runway_with_time_remaining() {
        let now = 1_000_000;
        let reset = ResetAt::after(now, WindowKind::Weekly.duration_seconds() / 2);

        let healthy = UsageWindow::new(WindowKind::Weekly, 40.0, Some(reset)).unwrap();
        let behind = UsageWindow::new(WindowKind::Weekly, 60.0, Some(reset)).unwrap();
        let danger = UsageWindow::new(WindowKind::Weekly, 85.0, Some(reset)).unwrap();

        assert_eq!(Severity::for_window(&healthy, now), Severity::Normal);
        assert_eq!(Severity::for_window(&behind, now), Severity::Warning);
        assert_eq!(Severity::for_window(&danger, now), Severity::Danger);
    }

    #[test]
    fn low_quota_is_safe_when_reset_is_close() {
        let now = 1_000_000;
        let reset = ResetAt::after(now, WindowKind::Weekly.duration_seconds() / 10);
        let window = UsageWindow::new(WindowKind::Weekly, 85.0, Some(reset)).unwrap();

        assert_eq!(Severity::for_window(&window, now), Severity::Normal);
    }

    #[test]
    fn severity_is_unknown_without_a_current_reset_time() {
        let window = UsageWindow::new(WindowKind::Weekly, 85.0, None).unwrap();
        assert_eq!(Severity::for_window(&window, 1_000_000), Severity::Unknown);

        let expired = UsageWindow::new(
            WindowKind::Weekly,
            85.0,
            Some(ResetAt::from_unix_seconds(999_999)),
        )
        .unwrap();
        assert_eq!(Severity::for_window(&expired, 1_000_000), Severity::Unknown);
    }

    #[test]
    fn provider_aliases_are_explicit() {
        assert_eq!("claude-code".parse::<Provider>().unwrap(), Provider::Claude);
        assert_eq!("antigravity".parse::<Provider>().unwrap(), Provider::Agy);
        assert_eq!(Provider::Grok.badge(), "[X]");
        assert_eq!(Provider::Codex.icon(), "◈C");
        assert_eq!(Provider::Claude.icon(), "✦Cl");
    }

    #[test]
    fn opencode_maps_to_codex_and_grok_subscription_quota() {
        assert_eq!(
            Provider::providers_for_agent("opencode"),
            Some(vec![Provider::Codex, Provider::Grok])
        );
        assert_eq!(
            Provider::providers_for_agent("  OpenCode\t"),
            Some(vec![Provider::Codex, Provider::Grok])
        );
    }

    #[test]
    fn one_to_one_agent_kinds_keep_their_single_provider() {
        assert_eq!(
            Provider::providers_for_agent("claude"),
            Some(vec![Provider::Claude])
        );
        assert_eq!(
            Provider::providers_for_agent("claude-code"),
            Some(vec![Provider::Claude])
        );
        assert_eq!(
            Provider::providers_for_agent("ANTIGRAVITY-CLI"),
            Some(vec![Provider::Agy])
        );
        assert_eq!(
            Provider::providers_for_agent("codex"),
            Some(vec![Provider::Codex])
        );
        assert_eq!(
            Provider::providers_for_agent("grok"),
            Some(vec![Provider::Grok])
        );
        assert_eq!(Provider::providers_for_agent("gemini"), None);
    }

    #[test]
    fn multi_provider_kinds_have_no_single_provider_identity() {
        assert!("opencode".parse::<Provider>().is_err());
    }
}
