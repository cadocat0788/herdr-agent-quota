use herdr_agent_quota::dashboard::render_provider;
use herdr_agent_quota::model::{
    AccountBalance, Provider, ProviderSnapshot, ResetAt, UsageWindow, WindowKind,
};
use herdr_agent_quota::presentation::MetadataTokens;

#[test]
fn agent_row_renders_compact_reset_eta_without_absolute_timestamp() {
    let snapshot = ProviderSnapshot::new(
        Provider::Grok,
        vec![UsageWindow::new(
            WindowKind::Weekly,
            79.0,
            Some(ResetAt::from_unix_seconds(183_600)),
        )
        .unwrap()],
        1,
    );
    assert_eq!(
        render_provider(Provider::Grok, Some(&snapshot), 0),
        "Grok WARN\r\n  week 21% left reset 2d3h"
    );
}

#[test]
fn deepseek_row_renders_raw_balance_and_severity_symbol() {
    let snapshot =
        ProviderSnapshot::new(Provider::DeepSeek, vec![], 1).with_balance(Some(AccountBalance {
            currency: "USD".to_string(),
            total_balance: "4.07".to_string(),
            is_available: true,
        }));
    assert_eq!(
        render_provider(Provider::DeepSeek, Some(&snapshot), 0),
        "DeepSeek ●\r\n  4.07 USD"
    );
    let tokens = MetadataTokens::from_snapshot(&snapshot, 0);
    assert_eq!(tokens.quota_summary, "4.07 USD");
    assert_eq!(tokens.quota_state, "●");
    assert!(tokens.quota_5h.is_empty());
    assert!(tokens.quota_week.is_empty());
}
