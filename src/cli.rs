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
}

impl ProviderSelection {
    pub fn providers(self) -> Vec<Provider> {
        match self {
            Self::All => Provider::ALL.to_vec(),
            Self::Codex => vec![Provider::Codex],
            Self::Grok => vec![Provider::Grok],
            Self::Claude => vec![Provider::Claude],
            Self::Agy => vec![Provider::Agy],
        }
    }
}
