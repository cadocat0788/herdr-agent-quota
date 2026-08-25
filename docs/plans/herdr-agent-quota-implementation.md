# herdr-agent-quota implementation plan

Status: implemented and published
Plan date: 2026-08-15  
Implementation status: complete; local Herdr smoke checks, provider fixtures, release gates, and public repository publication passed

## 1. Outcome

Build a public MIT-licensed Rust plugin named `herdr-agent-quota` that shows the
active local account's subscription quota in Herdr:

- Codex: weekly remaining percentage.
- Grok/SuperGrok: weekly remaining percentage.
- Claude Code: five-hour and weekly remaining percentages.
- Agy/Antigravity: five-hour and weekly statusLine windows, conservatively
  aggregated across Gemini and third-party model pools.
- Herdr Agents sidebar: a readable provider/status row plus a quota row per agent.
- Optional read-only terminal pane: expanded quota details and diagnostics.

The plugin must be lightweight. It must not run a permanent resident daemon.
Refreshes are one-shot commands triggered by Herdr startup, selected agent
lifecycle events, or the user; one bounded global watcher may run only while an
agent is working. Between triggers, the last successful value remains visible
indefinitely.

## 2. Frozen product decisions

### Included in v1

- Rust implementation on macOS and Linux.
- One active local account per provider.
- Text provider names: `Codex`, `Grok`, `Claude`, and `Agy`.
- Remaining percentage is the primary number.
- Time-aware severity compares remaining quota percentage with the percentage of
  the 5-hour or 7-day window still remaining:
  - quota at or ahead of time progress: normal, symbol `●`.
  - quota behind time progress: warning, symbol `▲`.
  - quota behind time progress and below `20%`: danger, symbol `!`.
  - missing or expired reset time: unknown, symbol `?`.
- Static provider badge styling through Herdr `rows_by_agent`.
- Dynamic severity represented by readable `OK`, `WARN`, `LOW`, and `N/A` labels;
  legacy symbol tokens remain available for compatibility.
- Manual Herdr configuration and an optional `configure` helper with preview,
  backup, apply, and uninstall paths.
- A local cache containing normalized usage snapshots and bounded, truncated
  Codex session previews needed by the sidebar fallback.
- No telemetry, no upload of usage data, no browser Cookie access, and no stored
  provider credentials.
- Unknown or changed provider schemas fail safely. A failed refresh retains the
  last successful snapshot; if there has never been a success, the sidebar is not
  overwritten with a transient `unavailable` value.

### Explicitly excluded from v1

- A global custom card above or below the entire Herdr Agents list. Herdr v1 has
  no native non-terminal sidebar insertion point.
- SVG or PNG provider logos in the sidebar.
- Multiple accounts for one provider.
- xAI developer/API-team usage. Grok must show the SuperGrok weekly pool.
- Browser Cookie, Keychain, or web-page scraping fallbacks.
- A permanent polling daemon, OS service, or scheduled job.
- Resident minute-by-minute countdowns, `updated N minutes ago`, automatic stale
  transitions, alerts, notifications, or usage history.
- Windows support.
- Automatic login, token refresh, or credential migration for any provider.

## 3. Compatibility baseline

Implement and test against these current stable releases, checked on the plan date:

| Dependency | Pinned baseline | Contract used |
| --- | --- | --- |
| Herdr | [`v0.8.0`](https://github.com/herdrdev/herdr/releases/tag/v0.8.0) | plugin v1 manifest, CLI/socket schema, custom sidebar metadata |
| Codex CLI | [`0.147.0`](https://github.com/openai/codex/releases/tag/rust-v0.147.0) | `codex app-server --stdio`, `account/rateLimits/read` |
| Claude Code | [`v2.1.233`](https://github.com/anthropics/claude-code/releases/tag/v2.1.233) (local smoke test: `2.1.232`) | `statusLine` JSON `rate_limits` fields |
| Grok Build | source contract at [`eb267fe`](https://github.com/xai-org/grok-build/commit/eb267feff13129e568df38fb6fdf0ceb65f735d6) | CLI auth file and credits billing response |

The implementation is validated against these detected versions and fails closed
on malformed or changed response shapes; it does not reject a newer patch release
solely because its version string changed. Herdr sets `min_herdr_version = "0.8.0"`
in the manifest.

## 4. Host constraints that shape the design

Herdr plugin v1 runs executable commands declared in `herdr-plugin.toml`; it does
not provide an SDK or native sidebar component API. The supported path is:

1. Discover agent panes with Herdr's CLI/socket API.
2. Report string tokens with `pane.report_metadata` or
   `herdr pane report-metadata`.
3. Let the user reference those tokens from `[ui.sidebar.agents].rows` and
   `rows_by_agent`.

Custom token values are capped at 80 characters and a metadata report may update
at most 16 tokens. The plugin publishes the readable tokens below; the old
`$quota_badge` marker remains recognized for configuration cleanup but is no
longer sent as a separate metadata value:

- `$quota_badge`: retired cleanup marker; it is no longer published.
- `$quota_state`: `●`, `▲`, `!`, or `?`.
- `$quota_icon`: retired cleanup marker; native Herdr state plus provider name
  replaces this redundant value.
- `$quota_provider`: `Codex`, `Grok`, `Claude`, or `Agy` for custom layouts.
- `$quota_status`: retired cleanup marker; quota window rows carry the visible
  health color.
- `$quota_context`: provider-reported context-used percentage when available.
- `$quota_cache`: one-decimal cumulative cache hit rate for the main session
  transcript when a session boundary is available.
- `$quota_cache_ttl`: the remaining approximate provider TTL when supported;
  it shares the sidebar row with `$quota_cache`.
- `$quota_error`: short red diagnostics such as `no cached` when an estimated
  cache TTL has expired.
- `$quota_5h`: compact five-hour remaining value and reset ETA when exposed.
- `$quota_week`: compact weekly remaining value and reset ETA.
- `$quota_summary`: window-driven compact remaining values and reset ETAs.
- `$quota_topic`: latest user prompt extracted from recent pane output.
- `$quota_error`: short reason, intended for diagnostics rather than the default row.

Recommended compact values:

```text
● Owner · Grok
B-325 budget cap
7d 39% 2d3h
● Owner · Claude
refactor auth middleware
5h 42% 3h07m · 7d 73% 2d3h
```

The same provider-level snapshot is repeated on every matching agent pane. Token
updates use source `herdr-agent-quota`, a monotonic sequence number, and a one-day
TTL. Old successful values therefore remain until a later successful update.

## 5. Proposed project layout

```text
.
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
├── herdr-plugin.toml
├── rust-toolchain.toml
├── src
│   ├── main.rs
│   ├── cli.rs
│   ├── model.rs
│   ├── cache.rs
│   ├── refresh.rs
│   ├── herdr.rs
│   ├── dashboard.rs
│   ├── configure
│   │   ├── mod.rs
│   │   ├── herdr.rs
│   │   ├── claude.rs
│   │   └── agy.rs
│   └── providers
│       ├── mod.rs
│       ├── codex.rs
│       ├── grok.rs
│       ├── claude.rs
│       └── agy.rs
├── tests
│   ├── fixtures
│   │   ├── codex
│   │   ├── grok
│   │   ├── claude
│   │   └── agy
│   ├── provider_contracts.rs
│   ├── configure_round_trip.rs
│   └── metadata_rendering.rs
└── docs
    ├── plans
    │   └── herdr-agent-quota-implementation.md
    └── research
        └── codexbar-grok-usage.md
```

Prefer a single binary and ordinary modules. Do not create a workspace, plugin SDK
wrapper, generic provider framework, database, or async runtime unless a verified
contract makes one necessary.

## 6. Runtime model

### Commands

Keep the public surface small:

```text
herdr-agent-quota refresh [--provider codex|grok|claude|all] [--force] [--json]
herdr-agent-quota watch [--provider codex|grok|claude|agy|all] [--interval-seconds N]
herdr-agent-quota event
herdr-agent-quota dashboard
herdr-agent-quota configure --check
herdr-agent-quota configure --apply
herdr-agent-quota configure --apply --watch-interval-seconds N
herdr-agent-quota configure --uninstall
herdr-agent-quota claude-statusline
herdr-agent-quota agy-statusline
```

- `refresh`: fetch, normalize, cache, and publish quota tokens.
- `event`: read `HERDR_PLUGIN_EVENT_JSON`, identify affected provider(s), debounce,
  call the same refresh service, and start one global `watch` pulse for a
  working turn.
- `watch`: call `herdr agent list` once per configured interval, publish
  statusLine cache changes and debounced active fetches for every working
  provider, perform one final debounced pass when it settles, and exit after a safety
  cap if Herdr becomes unavailable. It never reads pane output.
- `dashboard`: render cached values and explicit unavailable reasons; pressing `r`
  may force a refresh, but the pane must not poll automatically.
- `configure`: manage Herdr rows and the Claude statusLine wrapper.
- `claude-statusline`: internal hook mode; parse stdin, update Claude's snapshot,
  then preserve the previous statusLine behavior.
- `agy-statusline`: one-shot hook mode; parse Agy's official `quota` object,
  cache it, and publish the sanitized snapshot.

### Refresh triggers

Declare one-shot startup and event hooks in `herdr-plugin.toml`:

- Startup: `refresh --provider all`.
- Events: `pane.agent_detected` and `pane.agent_status_changed`; refresh and read
  topics only for the provider named by the event, falling back to all providers
  when the event payload does not identify one.
- Focus: refresh only the selected provider, without reading pane content.
- Manual action: force refresh all providers.

Use a state-file timestamp and a cross-process lock to coalesce non-forced provider
refreshes occurring within 60 seconds. The active-turn watcher uses one global
coordination lock and a configurable 60-second poll interval (30 seconds to one
hour); a manual refresh bypasses the timestamp but still takes the lock. Do not
subscribe to raw output or `pane.updated`; those events are too frequent for quota
checks. The `pane.focused` path must stay provider-only,
must not read pane content, and must skip metadata reports while the pane is in
scrollback so it cannot recreate the original viewport feedback loop.

### Snapshot model

Use one normalized representation shared by adapters and renderers:

```text
ProviderSnapshot
  provider: Codex | Grok | Claude | Agy
  source: stable source identifier
  windows: Vec<UsageWindow>
  fetched_at: UTC timestamp, cache/diagnostics only

UsageWindow
  kind: FiveHour | Weekly
  used_percent: decimal 0..100
  remaining_percent: derived clamp(100 - used_percent)
  resets_at: optional UTC timestamp
```

Do not put credentials, raw responses, email addresses, account IDs, or refresh
tokens in this model. Percentages outside `0..=100`, non-finite values, and a Grok
period not explicitly marked weekly are contract errors.

### Cache and concurrency

- Store state under `HERDR_PLUGIN_STATE_DIR`; use the platform cache directory only
  when running outside Herdr.
- Use one versioned JSON file per provider plus a small debounce/sequence file.
- Write to a sibling temporary file and atomically rename it.
- Serialize concurrent hook executions with a lightweight file lock.
- On fetch failure, do not overwrite a successful snapshot.
- The UI contains no freshness text and snapshots do not expire automatically.

## 7. Provider contracts

### Codex

1. Verify `codex --version` is the pinned supported version.
2. Spawn `codex app-server --stdio` only for the duration of the refresh.
3. Perform the documented JSON-RPC initialization handshake.
4. Call `account/read`; reject API-key mode because it is not the requested ChatGPT
   subscription quota.
5. Call `account/rateLimits/read`.
6. Select the window whose declared duration is seven days; do not assume that
   `primary` or `secondary` always means weekly.
7. Normalize `usedPercent` and `resetsAt`, cache the result, then terminate the child
   cleanly. Enforce startup, response, and shutdown timeouts.

Codex v1 intentionally ignores the five-hour window in the UI because the approved
scope asks only for its weekly quota.

### Grok

1. Verify the installed `grok` is compatible with the reviewed Grok Build contract.
2. Resolve `GROK_HOME` when supported, otherwise read `~/.grok/auth.json`.
3. Select the active xAI login entry with a non-expired `key`; never read or log the
   refresh token.
4. Make one HTTPS request:

   ```text
   GET https://cli-chat-proxy.grok.com/v1/billing?format=credits
   Authorization: Bearer <login key>
   X-XAI-Token-Auth: xai-grok-cli
   Accept: application/json
   ```

5. Accept the value only when `config.currentPeriod.type` explicitly represents a
   weekly period. Read `config.creditUsagePercent` and `currentPeriod.end`.
6. Do not call xAI Management API, import Cookies, access browser Keychains, use the
   gRPC/WKE fallback, log a raw response, or refresh the login token.
7. A missing/expired login returns: `unavailable: run grok login`. A changed billing
   schema returns an unsupported-contract reason and leaves the old snapshot intact.

The endpoint is an official Grok CLI backend, not a public stability guarantee.
Keep its parser isolated and fixture-tested so it can be replaced surgically.

### Claude Code

1. Verify `claude --version` is the pinned supported version.
2. Install `claude-statusline` as a user-level Claude Code `statusLine` command only
   through `configure --apply`.
3. Read the statusLine JSON from stdin and extract:
   - `rate_limits.five_hour.used_percentage` and `resets_at`.
   - `rate_limits.seven_day.used_percentage` and `resets_at`.
4. Normalize and atomically cache both windows.
5. If a previous statusLine exists, forward the exact stdin payload to it and
   preserve its stdout, stderr, and exit behavior. If it cannot be represented or
   chained safely, `configure --apply` must stop before changing settings.
6. If there was no previous statusLine, emit no extra decorative output; the hook's
   purpose is collection for Herdr.

The plugin must not implement Claude.ai OAuth, read Claude credentials, or route
requests on the user's behalf.

### Agy/Antigravity

1. Use Agy's native `/statusline` command to feed one JSON payload to the
   one-shot `agy-statusline` hook; no resident process or external API call is
   required.
2. Read the official `quota` object keys `gemini-5h`, `gemini-weekly`, `3p-5h`,
   and `3p-weekly`, each carrying a `remaining_fraction` value.
3. When both Gemini and third-party pools are present, use the lowest remaining
   value for each window so one sidebar row never overstates available quota.
4. Cache only the normalized percentages and retain the previous snapshot when
   a statusLine payload is missing or changes shape.

## 8. Herdr presentation

### Pane discovery and metadata

- Use `HERDR_BIN_PATH`; do not assume `herdr` is on `PATH` inside hooks.
- Use `herdr agent list`/`pane list` JSON or the matching v0.8 socket methods to find
  canonical agent kinds `codex`, `grok`, and `claude`.
- For every matching pane, report the same provider snapshot as custom tokens.
- Use source `herdr-agent-quota` and a monotonically increasing sequence value.
- Clear only tokens owned by this source. Never alter semantic agent state, title,
  display agent, workspace, layout, or another reporter's metadata.

### Sidebar configuration

`configure --check` prints the exact TOML change but writes nothing. `--apply` must:

1. Locate the active Herdr config using the v0.8-supported path/CLI.
2. Parse and edit with a format-preserving TOML editor.
3. Back up the original once with a deterministic adjacent name.
4. Retain Herdr's official `state_icon`/`tab` row (without the directory),
   add the plugin-owned `$quota_topic` as its own row, add
   `$quota_provider` to the plane row, and add one compact row containing the
   styled `$quota_5h` and `$quota_week` variants. Herdr elides missing tokens
   and their separators for weekly-only agents.
   The topic token is extracted from the latest user prompt on event refresh;
   it stays empty instead of falling back to AI status titles. This keeps every
   agent to three readable lines and shows the provider name only once. The
   helper adds brand-colored `rows_by_agent` projections for providers without
   an existing override and never replaces user-owned projections.
5. Be idempotent: a second apply produces no diff.
6. Validate the resulting config before replacement. The user can apply it to a
   running Herdr session with `herdr server reload-config`.
7. Record only the plugin-owned edit needed for a precise uninstall.

`--uninstall` removes only plugin-owned row additions and restores the previous
Claude statusLine definition. It must not replace the entire current config from an
old backup.

README must also provide the equivalent manual TOML snippet for users who do not
want automatic edits.

### Detail pane

Declare a `[[panes]]` entry that starts `dashboard`. Keep it read-only except for:

- `r`: force refresh.
- `q`: exit.

Render provider, window, remaining percentage, source, and the last failure reason.
Do not display reset/update times. The pane must not poll or mutate provider settings
by itself.

## 9. Dispatchable work packages

Complete packages in dependency order. Packages 2, 3, and 4 may run in parallel
after package 1. Assign one integration owner for shared files (`Cargo.toml`,
`main.rs`, manifest, README) to avoid merge conflicts.

### Package 0 — Repository and contract freeze

Owner scope: repository root and fixture inventory only.

- Initialize Git with default branch `main`.
- Add MIT license, `.gitignore`, Rust package skeleton, locked toolchain, and
  `herdr-plugin.toml` with `min_herdr_version = "0.8.0"`.
- Declare all dependencies needed by the approved design before parallel work.
- Capture sanitized provider response fixtures for the pinned versions; no real
  token, email, ID, or full private response may enter Git.
- Record exact local verification commands for all four upstream versions.

Verify:

```text
cargo metadata --locked --no-deps
cargo check --locked
```

### Package 1 — Domain model, cache, and CLI skeleton

Owner scope: `src/model.rs`, `src/cache.rs`, `src/cli.rs`, initial `src/main.rs`.

- Implement validated percentages, remaining calculation, time-aware severity,
  provider snapshots, atomic cache, lock, and 60-second debounce.
- Define the approved commands without provider logic.
- Unit-test on-pace, behind-pace, low-but-safe, danger, and missing-reset cases.
- Test that failed refreshes cannot replace a successful cache.

Exit criterion: provider adapters can be implemented behind a small function that
returns `ProviderSnapshot`; do not introduce a generic plugin framework.

### Package 2 — Codex adapter

Owner scope: `src/providers/codex.rs`, Codex fixtures/tests only.

- Implement the pinned app-server handshake, account-mode check, seven-day window
  selection by duration, timeouts, and child teardown.
- Cover valid weekly, missing weekly, API-key account, malformed JSON-RPC, timeout,
  and process-exit fixtures.

Exit criterion: fixture and local signed-in smoke tests return only the Codex weekly
snapshot and leave no child process running.

### Package 3 — Grok adapter

Owner scope: `src/providers/grok.rs`, Grok fixtures/tests only.

- Implement read-only auth discovery and the reviewed CLI-proxy request.
- Strictly require an explicit weekly period.
- Cover valid weekly, monthly period, missing type, expired/missing auth, 401/403,
  timeout, non-JSON, and schema-change cases.
- Add a test that captured HTTP requests never put credentials in URLs or errors.

Exit criterion: a signed-in `grok` account yields SuperGrok weekly remaining usage
without browser access or xAI developer API data.

### Package 4 — Claude adapter and reversible statusLine hook

Owner scope: `src/providers/claude.rs`, `src/configure/claude.rs`, Claude fixtures.

- Parse both approved rate-limit windows from stdin.
- Preserve and chain an existing statusLine command byte-for-byte at the JSON input
  boundary.
- Implement check/apply/uninstall against temporary settings trees first.
- Test absent fields, one missing window, malformed stdin, existing hook chaining,
  repeated apply, and uninstall after unrelated settings edits.

Exit criterion: Claude behavior is unchanged when an old statusLine exists, while
both usage windows reach the cache after an official statusLine update.

### Package 5 — Herdr bridge and event refresh

Depends on: packages 1–4.

Owner scope: `src/herdr.rs`, `src/refresh.rs`, event fixtures, manifest hooks.

- Discover provider panes from v0.8 JSON output.
- Map cached provider snapshots to the readable provider/status/summary tokens.
- Report metadata to every matching pane without altering semantic state.
- Implement startup, approved event hooks, bounded active-turn pulses, force
  refresh, cross-process coalescing, monotonic sequence values, and partial
  provider failure.

Exit criterion: mocked Herdr commands prove exact argv/token values; a local Herdr
smoke test shows different provider quotas on matching agent rows.

### Package 6 — Herdr configure helper

Depends on: packages 1 and 5. May overlap late provider work.

Owner scope: `src/configure/herdr.rs`, config round-trip tests.

- Implement preview, surgical apply, validation, backup, idempotence, and uninstall.
- Generate provider-specific badge styling and quota rows.
- Preserve comments, ordering, unknown keys, and unrelated edits.

Exit criterion: golden before/after fixtures round-trip cleanly and two consecutive
applies produce identical files.

### Package 7 — Read-only dashboard pane

Depends on: packages 1 and 5.

Owner scope: `src/dashboard.rs`, pane manifest entry, renderer snapshots.

- Render all provider windows, sources, and unavailable reasons; do not show times.
- Add only `r` and `q`; do not add a timer loop.
- Handle narrow terminals and missing providers without panics or clipped control
  sequences.

Exit criterion: snapshot tests cover full, partial, unavailable, and narrow views.

### Package 8 — Integrated verification

Depends on: packages 2–7.

- Run all static and unit checks.
- Test a fresh install, manual config, automatic config, repeated config, and
  uninstall in isolated temporary homes.
- Smoke-test real signed-in Codex, Grok, Claude Code, and Agy accounts without recording
  their values.
- Verify startup and each declared Herdr event hook.
- Kill/exit every short-lived child and confirm no `herdr-agent-quota`, Codex
  app-server, dashboard, or settled-turn watcher remains after the relevant
  command ends.
- Inspect logs and fixtures for bearer tokens, refresh tokens, Cookies, emails, and
  account IDs.

Required clean gate:

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
```

### Package 9 — Documentation, repository publication, and marketplace

Depends on: package 8 passing.

- README first sentence:
  `Show Claude Code, Codex, Grok, and Agy subscription usage in Herdr's agent sidebar.`
- Explain exact data sources, remaining-percentage semantics, single-account scope,
  bounded active-turn behavior, retained old values, configuration, uninstall, and provider
  failure messages.
- Include a real screenshot with no private workspace or account data.
- GitHub description:
  `Live Claude Code, Codex, Grok, and Agy quotas in Herdr: provider names, 5-hour/weekly remaining percentages, and color-coded sidebar warnings.`
- Create the public repository `herdr-agent-quota` under the authorized account only
  after GitHub CLI authentication is repaired and the owner confirms the final diff.
- Add no more than 20 topics. Required/recommended:
  `herdr-plugin`, `herdr`, `rust`, `ai-agents`, `coding-agents`, `agent-usage`,
  `quota-monitor`, `rate-limit`, `claude-code`, `codex`, `grok`, `xai`, `agy`,
  `antigravity`, `gemini`, `sidebar`, `statusline`.
- Push `main`, confirm the root `herdr-plugin.toml`, and verify marketplace discovery
  after its documented indexing interval.

Release only when the integrated gate is green and README limitations match actual
behavior.

## 10. Parallel dispatch map

```text
Package 0
   ↓
Package 1
   ├── Package 2: Codex
   ├── Package 3: Grok
   └── Package 4: Claude
             ↓
          Package 5: Herdr bridge
          ├── Package 6: configure
          └── Package 7: dashboard
                     ↓
                  Package 8: integration
                     ↓
                  Package 9: publish
```

Recommended dispatch: one integration owner handles packages 0, 1, 5, 8, and 9;
four provider owners handle packages 2–5; packages 6–7 can be assigned after their
dependencies settle. Provider owners must not edit shared manifests or dependency
files directly—send requested dependency changes to the integration owner.

## 11. Definition of done

The project is done only when all of the following are true:

- A real Herdr v0.8.0 session shows provider names and status labels on matching
  two-line agent rows after configuration.
- Codex and Grok show weekly remaining percentage; Claude and Agy show five-hour and
  weekly remaining percentages when supplied by their statusLine payloads.
- Values come from the approved subscription sources, not token counts or developer
  API billing.
- With no further events, the last successful usage remains displayed unchanged.
- A manual refresh updates all available providers and does not erase old successes
  when one provider fails.
- No permanent background service exists; one active-turn watcher is bounded and
  stops when all selected providers' agents settle.
- Config apply is previewable, idempotent, backed up, and precisely reversible.
- Existing Claude statusLine behavior survives apply and uninstall.
- No provider credential or browser Cookie is stored, logged, or committed.
- macOS and Linux verification gates pass.
- The public GitHub repository and Herdr marketplace metadata accurately describe
  the shipped behavior and limitations.

Implementation audit (2026-08-15): `cargo fmt --all -- --check`, `cargo clippy
--all-targets --all-features -- -D warnings`, `cargo test --all-targets
--all-features --locked`, and `cargo build --release --locked` pass. A Herdr
v0.8.0 session is linked to the plugin and a live refresh publishes provider
tokens to Claude, Grok, Codex, and Agy panes. The public repository is
[`levi-qiao/herdr-agent-quota`](https://github.com/levi-qiao/herdr-agent-quota);
Herdr marketplace indexing remains asynchronous after publication.

## 12. Primary references

- [Herdr plugin v1](https://herdr.dev/docs/plugins/)
- [Herdr configuration and sidebar rows](https://herdr.dev/docs/configuration/)
- [Herdr CLI reference](https://herdr.dev/docs/cli-reference/)
- [Herdr socket API](https://herdr.dev/docs/socket-api/)
- [Herdr marketplace](https://herdr.dev/docs/marketplace/)
- [Codex app-server](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
- [Claude Code statusLine](https://code.claude.com/docs/en/statusline)
- [Antigravity `/usage` and statusLine quota](https://antigravity.google/docs/cli/commands/usage?app=antigravity)
- [xAI Grok weekly usage FAQ](https://docs.x.ai/grok/faq)
- [Local CodexBar/Grok investigation](../research/codexbar-grok-usage.md)
- [CodexBar](https://github.com/steipete/CodexBar)
- [Grok Build](https://github.com/xai-org/grok-build)
