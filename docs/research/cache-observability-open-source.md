# Cache 命中率、TTL 与 context 可观测性调研

研究日期：2026-08-22（Asia/Shanghai）
范围：本仓库的 Codex、Claude Code、Agy/Antigravity、Grok Build；同时检查了几个开源 statusline/HUD 实现。
目标：找出可以在不发送模型请求、不重新登录、不 resume 活动会话的前提下显示的指标，并区分“可观测事实”和“本地估算”。

## 结论先行

1. **context 百分比可以按 provider 原生字段显示，并建议按 0–50 / 50–80 / 80+ 分成正常、警告、危险三色。** Claude/Agy 的 `used_percentage` 由 CLI 计算，Codex 的活动会话 token-usage 事件带 `last.totalTokens/modelContextWindow`，Grok 的 status/headless 数据也有当前 context token。不要在共享渲染器里重新推导四家百分比，否则会混用不同口径。
2. **provider 只给最近一次请求时，session 命中率必须由本地会话记录累计。** Claude/Agy 的 `current_usage` 是 latest-request 口径；Claude 的 statusLine 同时给出 `transcript_path` 和 `session_id`，插件按 transcript 字节 offset 增量解析 assistant usage，把整个主会话的 fresh/read/create 累计后再展示。Grok Build 已有 `session_usage` 累计字段；Codex app-server 也有 `total`。没有可靠 session 边界就隐藏累计百分比，不把某一轮冒充整个 session。推荐公式：

   ```text
   hit_percent = cache_read / (fresh_input + cache_creation + cache_read) * 100
   ```

   `cache_creation` 是写入，不算命中；把它放进分母比 `read / (fresh + read)` 更诚实。分母为零时隐藏该字段。
   这条三段式适用于 Claude/Agy statusLine 与 Grok Build headless；OpenAI/Codex 的 `input_tokens` 已包含 cached 子集，应使用 `cached_tokens / input_tokens`（若另有 `cache_write_tokens`，再单独展示写入量）。

3. **“缓存还剩几分钟”只有 Claude 可以做一个近似显示，而且不应作为准确 expiry。** Claude API 返回 5m/1h 写入桶，官方说明缓存使用时会刷新，且 TTL 从请求开始计时；statusLine 本身没有 entry 的 `created_at`/`expires_at`。开源项目通过读取当前会话 transcript 的尾部推断 TTL 桶和最近 timestamp，但这仍是近似值，不能代表多个 cache breakpoint 的统一到期时间。
4. **Codex/OpenAI、Agy、Grok 当前没有可供本插件诚实显示的动态 cache-entry expiry。** OpenAI 的 `prompt_cache_options.ttl` 是请求策略（不是活动 cache entry 的剩余时间）；Agy/Grok status payload 只有 token 计数；Codex token-usage event 也没有 expiry timestamp。
5. **实现把 context 与 cache 诊断拆开，但 cache 命中率和剩余 TTL 共用一行。** `$quota_context` 紧跟 provider 名称；`$quota_cache` 显示整个主 session 的累计命中率（固定一位小数，避免 99.6% 被四舍五入成 100%）；`$quota_cache_ttl` 只显示 `ttl≈58m`。计数来自正在运行的 CLI/statusLine 输入和本地 transcript，不联网、不登录、不消耗模型 token。Claude 在 statusLine 同时提供明确 5m/1h bucket 时显示带 `≈` 的 TTL；证据不足就隐藏未知部分。

## 四个 provider 的可获取字段

| Provider | context | cache read/write | TTL/过期时间 | 安全来源 |
| --- | --- | --- | --- | --- |
| Claude Code | `context_window.used_percentage`；`context_window.context_window_size` | `context_window.current_usage.input_tokens`、`cache_creation_input_tokens`、`cache_read_input_tokens` | API usage 有 `cache_creation.ephemeral_5m_input_tokens` / `ephemeral_1h_input_tokens`，但 statusLine 没有 entry expiry；可由 transcript 做近似 countdown | 当前 statusLine 的 stdin JSON |
| Agy/Antigravity | `context_window.used_percentage`、`context_window.context_window_size` | 同名 `current_usage` 字段 | 官方 statusLine 没有 TTL/expiry 字段 | 当前 statusLine 的 stdin JSON |
| Codex | app-server `thread/tokenUsage/updated` 的 `last.totalTokens` / `modelContextWindow` 可计算；必须关联活动 `threadId` | app-server schema 的 `cachedInputTokens`、`cacheWriteInputTokens` | event/schema 无 expiry；OpenAI API TTL 是请求级策略 | 活动 app-server 事件；临时独立 app-server 无法读取活动 thread 的实时事件 |
| Grok Build | 官方 status-line 的 `context_window.context_tokens` / `used_percentage` | status-line `context_window.session_usage.{input_tokens,cache_creation_input_tokens,cache_read_input_tokens}` 是 session 累计；headless 也报 `cache_read_input_tokens` / `cache_creation_input_tokens` | 当前 xAI status/headless contract 无 cache-entry TTL | 当前会话 event/status 输出；不要为了 HUD 额外启动模型 turn |

官方字段依据：

- [Claude statusLine 可用字段与 context 口径](https://code.claude.com/docs/en/statusline#available-data) 明确给出 `current_usage` 四类 token，并说明 `used_percentage` 只计算 input + cache creation + cache read；statusLine 命令在本地运行且不消耗 API token。
- [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) 说明默认 5 分钟、可选 1 小时、命中会刷新 TTL、TTL 从请求开始计时，并给出 `cache_creation` 的 5m/1h 分桶。
- [Antigravity statusLine schema](https://antigravity.google/docs/cli/statusline/#available-json-fields) 的示例包含 context window、cache read/create 和 quota reset；没有 cache expiry 字段。
- [Codex `ThreadTokenUsageUpdatedNotification` schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/schema/json/v2/ThreadTokenUsageUpdatedNotification.json) 包含 `last`、`total`、`modelContextWindow` 及 `cachedInputTokens`、`cacheWriteInputTokens`；没有 TTL。
- [Grok headless usage contract](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/14-headless-mode.md) 明确区分 uncached `input_tokens`、`cache_read_input_tokens` 和 `cache_creation_input_tokens`；[Grok normalized TokenUsage](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-sampling-types/src/conversation.rs) 也保留 cache-read/write 语义。

说明：上表的 Grok status-line 字段属于当前 Grok Build `main` 合同；本仓库现有 Grok adapter 仍只调用 CLI billing endpoint，因此这些 context/cache 字段在本插件里暂不显示，除非后续显式接入 Grok 的 status-line 配置。

### 口径和 TTL 的补充细节

- **Claude**：`input_tokens` 是最后一次请求中没有从 cache 读取、也没有写入 cache 的部分；`cache_creation_input_tokens` 是写入部分；`cache_read_input_tokens` 是命中部分。因此单轮分母是三者之和。Claude Code 的 statusLine 不提供 session 累计，插件只读 `transcript_path`，按 `session_id + transcript_offset` 累加主会话中的 assistant usage；首个 payload 会扫描一次已有 transcript，后续只读新增完整行。官方 OTel 仍可提供外部 collector 的逐请求指标，但插件不为此联网。
- **Codex/OpenAI**：Responses API 的 `input_tokens` 是总输入，`input_tokens_details.cached_tokens` 是其中的命中子集；usage 还可能提供 `cache_write_tokens`。所以 Codex 的最新请求命中率可用 `cached / input`；不要把 Claude 的三段式公式套过来。当前 OpenAI 文档把两类策略分开：GPT-5.6 的 `prompt_cache_options.ttl` 只有 `30m`，从写入开始计时，复用会刷新 eligibility，服务可以保留更久；支持旧 retention 的模型则用 `prompt_cache_retention`（`in_memory` 通常空闲 5–10 分钟、最多 1 小时，或扩展的最长 24 小时），这些都是策略/上限而非活动 entry 的实际 expiry 或命中保证。Codex app-server event 没有 `created_at`/`expires_at`；其 `cacheWriteInputTokens` 虽在 schema 中存在，却可能因 Codex 当前 Responses 解析链路丢弃 GPT-5.6 的 `cache_write_tokens` 而保持 0，因此只能把写入量当可选诊断，不把 0 解释成“没有写入”。
- **Agy/Gemini**：Google Gemini API 的 implicit cache 只在响应 `usage.total_cached_tokens`（或旧 API 的 `usage_metadata.cached_content_token_count`）报告命中数，没有 TTL 或 entry id。显式 `CachedContent` 资源另有 `createTime`、`updateTime`、`expireTime`，但它要求独立 Gemini API/Vertex 凭证；Antigravity OAuth/Code Assist 的 statusLine 不暴露这些资源元数据。不能把显式 cache 的过期时间套到交互会话上。并且官方 CLI 已把 SQLite（`.db`/`.db-wal`）作为会话格式，statusLine 示例里的 `transcript_path` JSONL 不能当作稳定的公开 usage schema；只有能解析到明确 `assistant` usage 行时才做 session 累计，否则退回 latest-request 计数。
- **Grok/xAI**：官方 API 的 cached token 是总 prompt 的子集，命中率可用 `cached_tokens / prompt_tokens`。xAI 明确写明缓存会因内存压力和请求路由变化而随时淘汰，没有公开 TTL；`x-grok-conv-id`/`prompt_cache_key` 只能改善路由和命中率，不能提供 expiry。Grok Build status-line 另提供 `context_window.session_usage` 的 `input_tokens`、`cache_creation_input_tokens`、`cache_read_input_tokens`，三者累计到 session 输入；因此可计算 session 命中率 `read / (input + creation + read)`，但这不是 provider 的 cache-entry 状态。Grok Build headless 的 `input_tokens` 采用 uncached 口径，另报 `cache_read_input_tokens`/`cache_creation_input_tokens`，需要按其 own output contract 计算。

### Agy headless 的额外边界

Antigravity CLI v1.1.7 起在 `json`/`stream-json` usage 中增加 `cache_read_tokens`，但 headless 模式会真正执行一次 prompt；它不能被本插件的 watcher 当作“无成本查询”。交互 statusLine 已经把 cache read/create 作为输入 payload，优先使用后者。

来源：[Claude OTel usage](https://code.claude.com/docs/en/monitoring-usage)、[OpenAI prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching)、[Gemini context caching](https://ai.google.dev/gemini-api/docs/generate-content/caching)、[Gemini caching API](https://ai.google.dev/api/caching)、[xAI cache behavior](https://docs.x.ai/developers/advanced-api-usage/prompt-caching/how-it-works)、[xAI usage and pricing](https://docs.x.ai/developers/advanced-api-usage/prompt-caching/usage-and-pricing)、[Grok Build status-line available data](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/25-status-line.md#available-data)、[Antigravity headless mode](https://antigravity.google/docs/cli/headless/)、[Antigravity CLI changelog](https://github.com/google-antigravity/antigravity-cli/blob/main/CHANGELOG.md)、[Antigravity issue: transcript schema is not public](https://github.com/google-antigravity/antigravity-cli/issues/423)。

## 可借鉴的开源实现

### 1. Claude cache hit：只读 statusLine，零网络

[`vfmatzkin/claude-statusline`](https://github.com/vfmatzkin/claude-statusline) 直接读取 stdin 的 `current_usage`，把 `cache_read_input_tokens` 与输入总量换算成 `cache 96%`；不启动后台进程，也不调用 API。适合作为本项目默认方案：在现有 Claude/Agy wrapper 中解析字段即可。

[`waelmas/claude-stat`](https://github.com/waelmas/claude-stat) 也从 statusLine 读取 context，并通过 transcript 累加 `cache_read_input_tokens`、`cache_creation_input_tokens` 与 fresh input，计算跨 turn 的命中比例。它用文件 size 做缓存，典型 repaint 开销约 5–20ms，而且没有后台 daemon 或网络请求。这里的“跨 turn”是本地统计，不是 provider 返回的单个 session 命中率；要避免重复计数，必须按 transcript offset/size 或 `prompt_id` 去重。

推荐借鉴：

- 默认展示 session 累计的 raw read/write/fresh-derived hit%；首个 session statusLine 调用会读取一次 transcript 建立基线，之后使用 `session_id + transcript_offset` 只读取新增完整行，不重复统计。
- `current_usage` 在首个响应及 `/compact` 后可能为 `null`；保持上次有效值，不能写成 0。

这里的“session 累计”只覆盖 statusLine `transcript_path` 指向的主会话。Claude Code 的 Task/subagent 可能另有 JSONL 文件；除非明确遍历并关联这些文件，否则不要把主 transcript 的合计声称为全任务树总量。

### 2. Claude TTL：可做近似，但不要伪装成 expiry

[`ilia-pluzhnikov/claude-code-statusline`](https://github.com/ilia-pluzhnikov/claude-code-statusline) 是目前看到的最完整方案：

- 计数直接来自 statusLine `current_usage`；
- 只读取 transcript 尾部约 16 KiB；
- 从最近的 assistant usage 记录中识别 `cache_creation.ephemeral_5m_input_tokens` 或 `ephemeral_1h_input_tokens`；
- 用记录 timestamp + 5m/1h 渲染一个倒计时；
- 配合 `refreshInterval: 60`，避免 idle 时倒计时冻结。

其文档同时承认 stdin 没有 TTL bucket 和 per-message timestamp，因此这是估算，不是服务端 expiry。Anthropic 官方还说明 TTL 从请求开始计时，并且每次命中都会刷新；长响应、多个 breakpoint、compact、模型/工具切换都会让“最后一条 assistant timestamp + TTL”偏离真实条目状态。本项目只在证据齐全时默认显示这个带 `≈` 的提示，其他情况隐藏。

另一种实现是 [`leeguooooo/claude-code-usage-bar`](https://github.com/leeguooooo/claude-code-usage-bar)：按最多 320 KiB 的尾部窗口反向读取 JSONL，分别记录最新 assistant timestamp 与最近一个 cache-write bucket，再计算倒计时；该项目在 bucket 缺失时采用 5 分钟保守回退。回退有助于始终显示状态，但会把未知的服务端策略伪装成 5m，因此本插件继续选择“无 bucket 就不显示 TTL”，只借鉴其 bounded tail + newest assistant anchor。

### 3. Codex 本地 usage：可统计命中率，但当前不适合实时 watcher

[`razzededge/codex-usage-audit`](https://github.com/razzededge/codex-usage-audit) 只读本地 Codex rollout/session JSONL，统计 input、cached input、cache write、output 及 context pressure；它使用私有 aggregate cache，未变化的 rollout 不重复解析，并且运行时不联网。

[`harveyxiacn/codex-usage-monitor`](https://github.com/harveyxiacn/codex-usage-monitor) 采用类似策略，读取 `~/.codex/sessions` 的 `token_count` 事件，计算 `cached_input_tokens / input_tokens` 和最新请求 context fill。

这些项目证明“本地 JSONL 能做历史命中率”，但不应直接复制到本插件的每分钟全局 watcher：

- rollout 文件含会话内容，扫描/解析有隐私和 I/O 成本；
- 每个 pane 对应的活动 thread/session 关联不稳定；
- token_count 是 turn 事件，和 app-server `thread/tokenUsage/updated` 的实时事件不是同一订阅；
- 用户明确要求不 resume、不扫描 rollout 时，应保持当前 Codex `thread/list` 仅用于会话预览，不新增 rollout 扫描。

长期可选方向是：由 Codex 官方 hook/长连接提供 event-driven token usage，再把事件按 `threadId` 写入本地 cache；没有可靠关联前不要猜 context 或命中率。

### 4. Grok/Agy HUD：本地读取可用，但能力边界不同

[`xiyouMc/grok-hud`](https://github.com/xiyouMc/grok-hud) 说明 Grok 没有 Claude 风格的原生 statusLine API，因此外部 HUD 读取 `~/.grok` 的 `signals.json`、`summary.json`、`updates.jsonl`；context 来自 signals，billing API 结果约 60 秒缓存。它没有通用 cache TTL，也不建议为了显示数据启动新的模型请求。

[`weby-homelab/antigravity-cli-statusline`](https://github.com/weby-homelab/antigravity-cli-statusline) 直接消费 Antigravity stdin payload，显示 context/session/current-turn tokens 和 quota；没有额外网络调用。Agy 的官方 stdin 已经给出 cache read/create，因此本项目无需仿照外部 server polling。

## 推荐实现分层

### 默认层（建议现在实现）

1. `ContextUsage` 保持 provider 原生百分比；用独立的紫色强调色与额度健康色区分，
   不把四家不同口径的 context 百分比重新套成统一危险阈值。
2. `CacheUsage` 使用可选字段：`fresh_input_tokens`、`read_tokens`、`creation_tokens`、`hit_percent`。
3. Claude/Agy 从当前 statusLine payload 解析；Grok/Codex 只有在当前安全链路拿到官方字段时才显示，缺失就隐藏。
4. Herdr metadata report 有 16-token 上限；当前固定保留 16 个动态 token，淘汰旧的 `$quota_icon`/`$quota_status` 发布位，并将 `$quota_cache` 与 `$quota_cache_ttl` 纳入固定预算。报告名始终不超过 16，旧 token 只做有界清理。
5. 不把 quota reset 当作 cache expiry。

### 有界 TTL 诊断层

本项目采用 ilia 项目的 bucket/timestamp 口径：session 首次建立累计时读取 transcript，之后每次只读新增字节；TTL token 显示
`ttl≈54m` 这类带剩余时间的本地估算（5m/1h bucket），不显示“expires in”或最近发送时间；归零后改显示红色 `no cached`。
任何解析失败、compact、字段缺失都不生成新估算；跨 session 不继承旧 TTL，避免误报。该扫描发生在已有 statusLine hook 内，不是全局 watcher，也不读取 pane。

### 明确不做

- 不为查询 cache 而调用 API、重新登录、发起 warm-up/model 请求；
- 不 resume 活动 Codex thread；
- 不把 quota `resets_at/reset_time` 伪装为 cache expiry；
- 不对每个 pane 启动独立 watcher 或重复读取 pane；
- 不扫描 Codex rollout 或其他未关联的 session JSONL；Claude/Agy 只在 statusLine
  提供主 transcript 时首次建立一次累计基线，后续按 offset 增量读取。

## 风险与验证重点

- **重复统计**：statusLine 可能在同一 turn 重绘多次；累计值必须用 `prompt_id`、transcript offset 或内容指纹去重，不能每次调用都加总。
- **口径漂移**：Claude 官方 `used_percentage` 是 input-only；Codex TUI 还有 baseline；Grok/Agy 可能由自身 CLI 计算。保存原始 provider 字段和 `source`，不要在 renderer 中二次“统一计算”。
- **缓存不是单一条目**：一个请求可有多个 breakpoint 和不同 TTL；任何倒计时都只能是近似提示。
- **compact/首轮空值**：`current_usage=null` 或字段缺失时保留旧值，避免侧栏闪烁和 metadata repaint。
- **性能**：每次 statusLine 只解析当前 JSON；Claude/Agy session 首次需要读取一次主 transcript（这是得到“整个 session”总量的必要成本），之后按字节 offset 只读取新增行，且同一 state 目录用文件锁避免多 pane 重复写入。不会启动 daemon、登录、发模型请求、读取 pane；全局 watcher 和 60 秒默认轮询路径不变。

## 参考来源

- [Claude statusLine data and local execution](https://code.claude.com/docs/en/statusline#available-data)
- [Anthropic prompt caching: TTL, refresh, usage fields](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [Antigravity statusLine schema](https://antigravity.google/docs/cli/statusline/#available-json-fields)
- [Codex app-server token usage schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/schema/json/v2/ThreadTokenUsageUpdatedNotification.json)
- [Codex token usage fields](https://github.com/openai/codex/blob/main/codex-rs/exec/src/exec_events.rs)
- [Codex issue: `cache_write_tokens` dropped from usage events](https://github.com/openai/codex/issues/32479)
- [Grok headless usage fields](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/14-headless-mode.md)
- [Grok status-line available data](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/25-status-line.md#available-data)
- [Grok normalized token usage](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-sampling-types/src/conversation.rs)
- [waelmas/claude-stat](https://github.com/waelmas/claude-stat)
- [ilia-pluzhnikov/claude-code-statusline](https://github.com/ilia-pluzhnikov/claude-code-statusline)
- [leeguooooo/claude-code-usage-bar](https://github.com/leeguooooo/claude-code-usage-bar)
- [vfmatzkin/claude-statusline](https://github.com/vfmatzkin/claude-statusline)
- [razzededge/codex-usage-audit](https://github.com/razzededge/codex-usage-audit)
- [harveyxiacn/codex-usage-monitor](https://github.com/harveyxiacn/codex-usage-monitor)
- [xiyouMc/grok-hud](https://github.com/xiyouMc/grok-hud)
- [weby-homelab/antigravity-cli-statusline](https://github.com/weby-homelab/antigravity-cli-statusline)
