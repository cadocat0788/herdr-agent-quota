# CLI 额度窗口重置时间能力调研

> 研究日期：2026-08-15（Asia/Shanghai）  
> 范围：本仓库现有四个 provider（Codex、Grok、Claude Code、Agy/Antigravity）。  
> 来源约束：只使用供应商官方文档/官方 CLI 源码，以及本仓库已有 parser、fixture 和类型；没有用二手文章推断供应商字段。
> 产品决策：虽然 Codex 5h 窗口在能力上可推导，本次实施按需求仍只展示 Codex weekly。

## 结论先行

四个 CLI 都能在不同程度上提供“下次重置”信息，但当前插件没有把它渲染出来。最需要先修的不是 UI，而是把供应商的不同时间类型在 parser 边界统一成内部的绝对时间：

| CLI | 5h 窗口 | weekly 窗口 | 一手字段 | 当前实现结论 |
| --- | --- | --- | --- | --- |
| OpenAI Codex | `inferred`：官方 schema 支持按 `windowDurationMins` 描述窗口；仓库 fixture 使用 300 分钟 | `confirmed`：仓库 fixture 使用 10080 分钟，官方 schema 说明窗口时长与 reset 字段 | `resetsAt`，Unix epoch 秒（绝对时间） | 已实施 weekly-only，数字 reset 已归一化 |
| Claude Code | `confirmed` | `confirmed` | `rate_limits.*.resets_at`，文档为 Unix 秒，2.1.233 statusLine 实现也可输出 RFC 3339 | 两种时间类型均已归一化 |
| Grok CLI / Grok Build | `unsupported`：官方 credits config 只有当前 weekly/monthly period，没有 5h bucket | `confirmed` | `config.currentPeriod.end`，RFC 3339（绝对时间） | weekly `end` 已正确读取；不要从 weekly 猜造 5h |
| Agy / Antigravity | `confirmed`（仓库已有 `gemini-5h`/`3p-5h` 合同） | `confirmed`（仓库已有 `gemini-weekly`/`3p-weekly` 合同） | `quota[*].reset_time`（绝对时间）或可选 `reset_in_seconds`（相对时长） | 5h/weekly 绝对字段已读取；相对字段未读取 |

这里的状态含义是：

- `confirmed`：一手契约明确给出该窗口/字段，或供应商没有歧义且仓库已有对应合同。
- `inferred`：一手 schema 给出通用窗口机制，具体 5h 映射还要结合仓库 fixture/运行时观察，不能把字段位置当作契约。
- `unsupported`：当前一手 credits/statusline 合同没有这个窗口，不能为了统一 UI 编造它。
- `unknown`：没有足够的一手证据；本次四个 provider 没有落入此类。

## 实施前仓库的缺口

1. `UsageWindow` 只保存 `resets_at: Option<String>`，没有表达 Unix 秒、RFC 3339 和相对秒数的类型区别（[`src/model.rs`](../../src/model.rs#L85-L107)）。
2. `ProviderSnapshot::summary`/`sidebar_summary` 只拼百分比（[`src/model.rs`](../../src/model.rs#L133-L173)）；dashboard 也只调用 summary，不会显示时间（[`src/dashboard.rs`](../../src/dashboard.rs#L60-L70)）。
3. Herdr metadata token 目前只有 `quota_5h`、`quota_week`、`quota_summary`（[`src/model.rs`](../../src/model.rs#L226-L254)；发布位置见 [`src/herdr.rs`](../../src/herdr.rs#L104-L137)）。可以保持这个公共 interface 不变，但不应再由 provider 分支分别拼字符串；应由一个共享 presentation module 统一生成百分比和 reset 文本。
4. 实施前的 fixture 与运行时类型没有分开验证。现在 Codex 用 Unix 数字 fixture，Claude 同时覆盖数字与 2.1.233 statusLine 可输出的 RFC 3339 字符串。

## 各 CLI 证据与结论

### 1. OpenAI Codex

**官方能力：`confirmed` 的绝对 reset 字段，5h 具体映射为 `inferred`。**

Codex `app-server` 的官方 v0.147.0 文档在 `account/rateLimits/read` 中给出 `rateLimits` 对象；每个窗口有 `usedPercent`、`windowDurationMins` 和 `resetsAt`。官方 field notes 明确 `windowDurationMins` 是窗口长度，`resetsAt` 是下次重置的 Unix 秒时间戳（[OpenAI Codex app-server v0.147.0：Rate limits，L2044-L2075](https://github.com/openai/codex/blob/rust-v0.147.0/codex-rs/app-server/README.md#L2044-L2075)）。官方示例还把 earned reset 描述为 “Weekly + 5 hr”，说明产品层同时存在这两个窗口，但示例对象本身没有承诺 `primary`/`secondary` 哪一个是哪个窗口。

本仓库的 Codex fixture 给出了实际选择所需的 300 分钟（5h）和 10080 分钟（7d）窗口（[`tests/fixtures/codex/rate-limits-weekly.json`](../../tests/fixtures/codex/rate-limits-weekly.json#L4-L13)）。因此建议把“按 duration 映射”视为仓库已验证合同，把 5h 的上游稳定性标成 `inferred`，不要按对象位置猜测。

**当前 parser：** [`src/providers/codex.rs`](../../src/providers/codex.rs#L29-L58) 只保留 `duration >= 10_000` 的 weekly，并且 `resetsAt` 通过 `Value::as_str` 读取。官方数字 Unix 秒会被静默转换成 `None`；5h 被有意丢弃。也就是说，Codex **可以支持 weekly 的精确 reset，也能按官方 schema 支持 5h，但当前代码尚未支持这两件事的完整组合**。

**实现边界：**

- parser 只负责把 `windowDurationMins == 300`/`10080`（以及未来明确等价值）映射到 `FiveHour`/`Weekly`，并把 Unix 秒归一化；不要把 `primary`/`secondary` 写死。
- 看到未知 duration 时保留为 unsupported/diagnostic，不要把它误标为 weekly。
- 官方没有 `reset_in_seconds` 字段；相对时长只能由统一 renderer 用 `reset_at - now` 计算。

### 2. Claude Code

**官方能力：5h 和 weekly 都是 `confirmed` 的绝对 reset。**

Claude Code 官方 statusLine 文档列出：

- `rate_limits.five_hour.used_percentage` 和 `rate_limits.seven_day.used_percentage` 是两个窗口的已用百分比；
- `rate_limits.five_hour.resets_at` 和 `rate_limits.seven_day.resets_at` 是窗口重置时刻的 Unix epoch 秒；
- 这些字段只在 Claude.ai 订阅用户完成首次 API 响应后出现，因此缺字段是合法的 unavailable 情况，而不是一定是解析错误（[Claude Code statusLine：Available data](https://code.claude.com/docs/en/statusline#available-data)，[Rate limit usage](https://code.claude.com/docs/en/statusline#rate-limit-usage)）。

仓库 parser 已分别读取 `five_hour` 和 `seven_day`（[`src/providers/claude.rs`](../../src/providers/claude.rs#L6-L30)），但 `resets_at` 仍然只接受字符串（[`src/providers/claude.rs`](../../src/providers/claude.rs#L42-L55)）。当前 fixture 中的 ISO 文本（[`tests/fixtures/claude/statusline-both.json`](../../tests/fixtures/claude/statusline-both.json#L3-L10)）不能覆盖官方 Unix 数字输入。

**结论：** Claude 两个窗口的 reset 能力是 `confirmed`，当前实现是“窗口可用、时间类型未适配”。应在 Claude adapter 内把数字 epoch 转成统一的绝对时间；保留缺失/null 为 `None`，不要把订阅尚未返回 rate limit 当作 0。

**实现边界：** 官方没有相对秒字段。renderer 统一计算剩余时长；如果 reset 已经过期，显示“即将重置”或 `0m`，不要显示负数。

### 3. Grok CLI / Grok Build

**官方能力：weekly 绝对 reset `confirmed`；5h `unsupported`。**

xAI 官方 Grok Build 源码把新 credits 响应建模为：`UsagePeriod` 的 `type`、`start`、`end`，并注明 start/end 是 RFC 3339，`type` 用于区分 weekly 与 monthly（[官方 `billing.rs`，L27-L40](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L27-L40)）。`BillingConfig` 将 `creditUsagePercent` 与 `currentPeriod` 作为新字段（[L55-L73](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L55-L73)）。官方 handler 调用 CLI proxy 的 `/billing?format=credits`，并发送登录态 Bearer token（[L188-L210](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L188-L210)）。官方测试 fixture 直接验证 `USAGE_PERIOD_TYPE_WEEKLY`、`currentPeriod.end` 的解析（[L520-L558](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L520-L558)）。

仓库 parser 已要求 `currentPeriod.type` 包含 `WEEKLY`，并读取 `currentPeriod.end`（[`src/providers/grok.rs`](../../src/providers/grok.rs#L85-L121)）；weekly fixture 也保留 RFC 3339 end（[`tests/fixtures/grok/credits-weekly.json`](../../tests/fixtures/grok/credits-weekly.json#L2-L9)）。

**结论：** Grok 能稳定展示 weekly 的绝对 reset；`end - now` 可以得到相对剩余时长，但这是本地派生，不是供应商提供的 relative 字段。当前 credits contract 没有 5h bucket，monthly period 也必须拒绝（现有 monthly fixture/测试已经覆盖），不能为了让四个 provider 对齐而伪造 `5h`。

**实现边界：** Grok adapter 只输出 `Weekly`；将 RFC 3339 `end` 解析为统一绝对时间。若未来官方增加 5h，新增独立字段映射和 fixture，不要在 renderer 中根据 provider 名称猜测。

### 4. Agy / Antigravity

**官方能力：5h、weekly 的绝对/相对 reset 都 `confirmed`，但 bucket key 仍应由 adapter 隔离。**

Google Antigravity 官方 statusLine schema 说明 `quota` 是 bucket ID 到状态对象的 map；每个状态包含 `remaining_fraction`、`reset_time`，并可选 `reset_in_seconds`（[官方 Status line customization：Available JSON fields](https://antigravity.google/docs/cli/statusline#available-json-fields)）。官方 plans 进一步说明 Pro/Ultra 计划存在“五小时刷新”和 weekly rate limit（[官方 Plans：Baseline Quota](https://antigravity.google/docs/plans#baseline-quota)）。

仓库将 `gemini-5h`/`3p-5h` 与 `gemini-weekly`/`3p-weekly` 隔离为常量（[`src/providers/agy.rs`](../../src/providers/agy.rs#L6-L8)），并在两个窗口分别取 Gemini/第三方池中的最低剩余值；reset_time 从被选中的最低池读取（[`src/providers/agy.rs`](../../src/providers/agy.rs#L43-L73)）。已有 fixture 同时覆盖四个 key 和绝对 reset 文本（[`tests/fixtures/agy/statusline-both.json`](../../tests/fixtures/agy/statusline-both.json#L2-L18)）。

**当前 parser 的差距：** 只读 `reset_time` 字符串，完全忽略官方可选的 `reset_in_seconds`。如果某个 bucket 只有相对字段，当前快照仍会有百分比但没有 reset。相对字段应作为绝对 reset 缺失时的 fallback；若两者同时存在，优先绝对时间，并可在测试中允许少量时钟误差。

**实现边界：** Agy 的 bucket 选择、最低池聚合和时间字段解析都应留在 Agy adapter；共享 renderer 不应知道 `gemini-*`/`3p-*`。

## 推荐的解耦实现方案（只做设计，不在本调研中改代码）

### 1. 在 parser 边界统一时间类型

目前 `Option<String>` 会把“数字 epoch”“RFC 3339”“相对秒”混在一起。建议新增一个小的、可序列化的内部值（名称可按项目风格调整）：

```text
ResetAt(i64)                 # Unix 秒，统一绝对时间
```

各 adapter 的职责保持单一：

| Adapter | 输入 | 归一化 |
| --- | --- | --- |
| Codex | `resetsAt` 数字；duration 映射窗口 | `at_unix_seconds` |
| Claude | `resets_at` Unix 数字或兼容旧字符串 | `at_unix_seconds` |
| Grok | `currentPeriod.end` RFC 3339 | `at_unix_seconds` |
| Agy | `reset_time` RFC 3339；无绝对值时用 `reset_in_seconds` | `at_unix_seconds = fetched_at + seconds` |

把相对秒在采集时转换成绝对秒，可以让缓存快照在下一次渲染时继续计算 ETA，也不需要把供应商原始 JSON 带入缓存。`Absolute | DerivedFromRelative` 对当前展示没有可观察差异，不建议进入公共模型；Agy adapter 内部知道 fallback 即可。兼容现有缓存时可先尝试旧字符串（RFC 3339/数字文本），写回新版本后再移除兼容分支。

不必为此新建通用 provider trait。现有四个 provider module 已经是真实 adapter，且 Codex/Grok 是主动 fetch，Claude/Agy 是 statusLine hook，强行抽成同一个浅 interface 只会增加间接层。它们共享的 seam 应是输出 `ProviderSnapshot<UsageWindow<ResetAt>>`。

### 2. 共享纯 formatter，provider 不参与显示逻辑

新增一个深的 presentation module，对外只需要类似 `format_window(window, now)` 和 `format_summary(snapshot, now)` 的小 interface。内部的 `format_reset_eta(reset_at, now)` 保持纯函数；时钟通过 `now` 传入，不在 formatter 里读系统时间。dashboard/sidebar/token 都跨这一个 seam：

- `remaining <= 0`：`reset due`，明确表示缓存窗口已到点，不显示负数；
- `< 60m`：`reset 45m`；
- `< 24h`：`reset 4h07m`，分钟固定两位；
- `>= 24h`：`reset 2d3h`，按需求只显示 day/hour；
- 缺失/解析失败：省略 reset 后缀，保留有效百分比，不在窄侧边栏里堆叠 `N/A`；
- 剩余正数秒先下取到整分钟，但最后 59 秒仍显示 `1m`；不做时区转换，因为内部只比较两个 Unix 时刻。

推荐最终一行保持紧凑：

```text
5h 42% 3h07m · 7d 73% 2d3h
```

侧栏省略重复的 `reset` 单词，并用 `7d` 缩写 weekly 窗口；dashboard 和
`$quota_summary` 仍可使用带 `reset`/`left` 的完整 formatter。

“有 d 就只显示 dh”会牺牲分钟精度，但符合窄 sidebar。dashboard 也应复用同一 formatter；如果未来真的需要完整本地时刻，再为 dashboard 扩展展示层，不让 provider parser 负责本地化。

### 3. 保持 token interface 小而稳定

第一版不需要增加 `$quota_5h_reset`/`$quota_week_reset`。让 presentation module 统一生成现有 `$quota_5h`、`$quota_week`和 `$quota_summary`，即可同时满足默认布局、dashboard 和已有自定义布局。只有当第二种布局真的需要独立 reset 时，再扩大 token interface。

这个 module 应根据 `snapshot.windows` 中实际存在的窗口按 `5h -> week` 排序，不再 `match Provider`来决定展示哪些窗口。因此 Codex 增加 5h 只是 adapter 多输出一个 window，Grok 仍只输出 week，presentation 代码不需要改。

### 4. 明确刷新语义

当前插件是事件触发、无常驻 daemon；metadata token 写入后 TTL 为一天，sidebar 不会自行每分钟重算（[`src/herdr.rs`](../../src/herdr.rs#L104-L114)；计划文档也明确排除 live countdown）。因此：

- dashboard 读取缓存时可用当前时间即时计算 ETA；
- sidebar 若要求数字实时跳动，需要新增受控的 refresh interval/事件，而不是在 formatter 中启动线程或 daemon；
- 若保持现有无轮询约束，sidebar 文案应理解为“上次发布时刻计算的剩余时间”，并在刷新事件后更新；这不是持续跳动的分钟级倒计时；
- 推荐第一版显示短 ETA，不显示 `updated N minutes ago`，避免把缓存新鲜度与 reset 语义混在一起。

## 建议落地顺序

1. **先修合同：** 把 Codex/Claude fixture 改为官方 Unix 数字；Codex 增加 300/10080 分钟双窗口断言，Agy 增加仅 `reset_in_seconds` 样例。验证目标是先让现有 parser 在真实 schema 测试下暴露失败。
2. **再建时间 seam：** 把 `UsageWindow.resets_at: Option<String>` 改为 typed `Option<ResetAt>`；Unix 数字直接归一化，RFC 3339 只在共享的小型解析 helper 中处理。现有依赖没有 RFC 3339 parser，实现时建议只增加一个轻量 `time` 依赖的 parsing 能力，不引入 async/runtime 或时区数据库。
3. **逐个改 adapter：** Codex 按 duration 只输出 week；Claude 读 Unix reset；Grok 严格解析 weekly RFC 3339 end；Agy 保留最低池聚合并增加 relative fallback。每个提交都可以用该 adapter 的纯 parser 测试独立验证。
4. **最后接展示：** 引入共享 presentation module，让 metadata 和 dashboard 通过同一 interface 生成文本；删除重复的 provider-specific summary 分支，但不改 Herdr 布局和 token 名。

这个顺序把 schema 正确性、内部模型、供应商 adapter 和 UI 变更分开；出问题时能直接定位在对应 module，不需要同时理解四家原始 JSON 与 Herdr 字符串布局。

## 建议的回归测试矩阵

1. **时间解析**：Codex/Claude 的数字 Unix 秒、旧 ISO 字符串兼容；Grok RFC 3339；Agy `reset_time` 与仅 `reset_in_seconds`。
2. **窗口选择**：Codex 300/10080 分钟各一条，交换 primary/secondary 顺序仍正确；未知 duration 不误标 weekly。
3. **Agy 聚合**：Gemini/3p 同窗口取最低剩余与对应 reset；最低池没有绝对值时回退相对秒。
4. **formatter 边界**：`reset 45m`、`reset 1h00m`、`reset 23h59m`、`reset 1d0h`、`reset 2d3h`、过去时间、缺失时间、非法时间。
5. **展示解耦**：dashboard 与 metadata token 使用同一 formatter；Grok 不产生 `5h`；旧 token 的百分比文本不被改义。

## 证据索引

- Codex：官方 [`app-server` v0.147.0 rate limits](https://github.com/openai/codex/blob/rust-v0.147.0/codex-rs/app-server/README.md#L2044-L2075)；仓库 [`codex` parser](../../src/providers/codex.rs#L29-L58) 与 [fixture](../../tests/fixtures/codex/rate-limits-weekly.json#L4-L13)。
- Claude：官方 [`statusLine` available data](https://code.claude.com/docs/en/statusline#available-data) 与 [rate limit usage](https://code.claude.com/docs/en/statusline#rate-limit-usage)；仓库 [`claude` parser](../../src/providers/claude.rs#L42-L55) 与 [fixture](../../tests/fixtures/claude/statusline-both.json#L3-L10)。
- Grok：xAI 官方 [`billing.rs` period model](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L27-L40)、[`BillingConfig`](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L55-L73)、[handler](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L188-L210)、[official test](https://github.com/xai-org/grok-build/blob/eb267feff13129e568df38fb6fdf0ceb65f735d6/crates/codegen/xai-grok-shell/src/extensions/billing.rs#L520-L558)；仓库 [`grok` parser](../../src/providers/grok.rs#L85-L121)。
- Agy：Google 官方 [`statusline` schema](https://antigravity.google/docs/cli/statusline#available-json-fields) 与 [Plans baseline quota](https://antigravity.google/docs/plans#baseline-quota)；仓库 [`agy` parser](../../src/providers/agy.rs#L6-L8) 与 [fixture](../../tests/fixtures/agy/statusline-both.json#L2-L18)。
