use crate::model::Provider;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "herdr-agent-quota",
    version,
    about = "Show AI agent subscription quota in Herdr"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Refresh {
        #[arg(long, default_value = "all")]
        provider: ProviderSelection,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Keep selected working providers' quotas fresh with one global poller.
    /// This is started automatically by the Herdr status event hook.
    Watch {
        #[arg(long, default_value = "all")]
        provider: ProviderSelection,
        /// Override the configured poll interval for this run.
        #[arg(long)]
        interval_seconds: Option<u64>,
    },
    Event,
    Focus,
    Dashboard,
    Status,
    Configure {
        #[arg(long, conflicts_with_all = ["apply", "uninstall"])]
        check: bool,
        #[arg(long, conflicts_with_all = ["check", "uninstall"])]
        apply: bool,
        #[arg(long, conflicts_with_all = ["check", "apply"])]
        uninstall: bool,
        /// Persist the active-turn poll interval while applying configuration.
        #[arg(long, requires = "apply")]
        watch_interval_seconds: Option<u64>,
    },
    ClaudeStatusline,
    AgyStatusline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ProviderSelection {
    All,
    Codex,
    Grok,
    Claude,
    Agy,
    // Derive would split the camel-case name into "open-code-go"; pin it to
    // the canonical source name with "go" as the short alias.
    #[value(name = "opencode-go", alias = "go")]
    OpenCodeGo,
    #[value(name = "deepseek")]
    DeepSeek,
}

impl ProviderSelection {
    pub fn providers(self) -> Vec<Provider> {
        match self {
            Self::All => Provider::ALL.to_vec(),
            Self::Codex => vec![Provider::Codex],
            Self::Grok => vec![Provider::Grok],
            Self::Claude => vec![Provider::Claude],
            Self::Agy => vec![Provider::Agy],
            Self::OpenCodeGo => vec![Provider::OpenCodeGo],
            Self::DeepSeek => vec![Provider::DeepSeek],
        }
    }
}
