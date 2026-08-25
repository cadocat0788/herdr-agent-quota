pub mod agy;
pub mod claude;
pub mod grok;
pub mod herdr;
mod statusline;

use crate::cache::CacheStore;
use anyhow::{Context, Result};

/// `--check` is also the no-flag default, so it needs no branch of its own.
pub fn run(
    _check: bool,
    apply: bool,
    uninstall: bool,
    watch_interval_seconds: Option<u64>,
) -> Result<()> {
    if apply || uninstall {
        std::env::var_os("HERDR_PLUGIN_STATE_DIR").context(
            "configuration writes must run through Herdr so every collector uses the same cache; invoke herdr-agent-quota.configure or herdr-agent-quota.uninstall",
        )?;
    }
    if uninstall {
        let cache = CacheStore::from_env()?;
        cache.stop_turn_watchers()?;
        grok::uninstall()?;
        agy::uninstall()?;
        claude::uninstall()?;
        herdr::uninstall()?;
        cache.clear_watch_interval()?;
    } else if apply {
        let cache = CacheStore::from_env()?;
        cache.clear_turn_watcher_stop()?;
        let interval = watch_interval_seconds.or_else(|| {
            std::env::var("HERDR_AGENT_QUOTA_WATCH_INTERVAL_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
        });
        let interval = if let Some(interval) = interval {
            cache.set_watch_interval_seconds(interval)?;
            interval
        } else {
            cache.watch_interval_seconds()
        };
        herdr::apply()?;
        claude::apply_with_refresh_interval(interval)?;
        agy::apply()?;
        grok::apply()?;
    } else {
        herdr::check()?;
        claude::check()?;
        agy::check()?;
        grok::check()?;
    }
    Ok(())
}
