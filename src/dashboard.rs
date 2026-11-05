use crate::cache::CacheStore;
use crate::model::{Provider, ProviderSnapshot};
use crate::presentation::{balance_summary, dashboard_summary};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::{self, IsTerminal, Write};
use std::time::Duration;

pub fn run() -> Result<()> {
    let cache = CacheStore::from_env()?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        print_snapshot(&cache)?;
        return Ok(());
    }
    enable_raw_mode()?;
    let result = interactive(&cache);
    let _ = disable_raw_mode();
    result
}

fn interactive(cache: &CacheStore) -> Result<()> {
    loop {
        print!(
            "{}",
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        );
        print!("{}", crossterm::cursor::MoveTo(0, 0));
        print_snapshot(cache)?;
        print!("\r\nr refresh  q quit\r\n");
        io::stdout().flush()?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => {
                        crate::refresh::run(&Provider::ALL, true, false)?;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn print_snapshot(cache: &CacheStore) -> Result<()> {
    print!("{}", render_snapshot(cache)?);
    Ok(())
}

fn render_snapshot(cache: &CacheStore) -> Result<String> {
    let mut output = String::from("Herdr Agent Quota\r\n=================\r\n");
    let now = CacheStore::now_unix();
    for provider in Provider::ALL {
        let snapshot = cache.load(provider)?;
        output.push_str(&render_provider(provider, snapshot.as_ref(), now));
        output.push_str("\r\n");
    }
    Ok(output)
}

pub fn render_provider(
    provider: Provider,
    snapshot: Option<&ProviderSnapshot>,
    now_unix: u64,
) -> String {
    match snapshot {
        Some(snapshot) => {
            let status = snapshot.severity(now_unix);
            let summary =
                balance_summary(snapshot).unwrap_or_else(|| dashboard_summary(snapshot, now_unix));
            let indicator = if snapshot.balance.is_some() {
                status.symbol()
            } else {
                status.label()
            };
            format!("{} {}\r\n  {}", provider.display_name(), indicator, summary)
        }
        None => format!("{} N/A\r\n  unavailable", provider.display_name()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ResetAt, UsageWindow, WindowKind};
    use tempfile::tempdir;

    #[test]
    fn renders_compact_remaining_values_with_reset_eta() {
        let snapshot = ProviderSnapshot::new(
            Provider::Claude,
            vec![
                UsageWindow::new(
                    WindowKind::FiveHour,
                    58.0,
                    Some(ResetAt::from_unix_seconds(14_820)),
                )
                .unwrap(),
                UsageWindow::new(
                    WindowKind::Weekly,
                    27.0,
                    Some(ResetAt::from_unix_seconds(183_600)),
                )
                .unwrap(),
            ],
            1,
        );
        let rendered = render_provider(Provider::Claude, Some(&snapshot), 0);
        assert_eq!(
            rendered,
            "Claude WARN\r\n  5h 42% left reset 4h07m · week 73% left reset 2d3h"
        );
    }

    #[test]
    fn snapshot_lines_return_to_column_zero_in_herdr_pty() {
        let directory = tempdir().unwrap();
        let rendered = render_snapshot(&CacheStore::new(directory.path())).unwrap();
        assert!(rendered.contains("Quota\r\n=================\r\nCodex"));
        assert!(!rendered.contains("Quota\n================="));
    }
}
