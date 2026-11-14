# QuotaDock Phase 3.5 Claude Findings

QuotaDock Phase 3.5 · 2026-10-08 ~ 10-09 · Windows 11 Home China 10.0.26200 · Claude Code 2.1.278 · Codex 0.162.0-alpha.2 · Tauri 2.12.1 · React 19.3

依据标记（沿用 §57）：

| 标记 | 含义 |
|---|---|
| `[LOCAL TEST]` | 本机实测，或本仓库的自动化测试 |
| `[REAL RUN]` | 本阶段的真实运行：release exe，连接本机真实的 Codex 和 Claude 账号 |
| `[ANTHROPIC DOCS]` | Anthropic 官方文档（code.claude.com/docs） |
| `[CLAUDE CODE OBSERVED]` | 本机 Claude Code 的实际输出，或其安装文件中的字符串 |
| `[CLAUDE CODE ISSUE]` | 社区 issue，**不是**官方契约 |
| `[QUOTADOCK HEURISTIC]` | QuotaDock 自己定的规则或阈值 |
| `[INFERENCE]` | 推断 |
| `[UNKNOWN]` | 未知 |

可行性门禁的详细过程见 [CLAUDE_DATA_SOURCE_FINDINGS.md](CLAUDE_DATA_SOURCE_FINDINGS.md)。本报告不重复其中的探测细节。

## 结论速览

| 项目 | 结果 |
|---|---|
| Workstream A：Codex detector-v2 | ✅ `reset-detector-v2`：idle→active 重新锚定分析段，派生起点标记为 derived / not authoritative，空闲快照不进入 burn。真实 fixture 回归测试通过（burn 2.24 → 12.9 pp/h）。**真实运行中也发生了一次**：22:45:50 Codex 5h 从 0% 变为 6%，v2 当场重锚，没有新 cycle，没有任何事件 `[REAL RUN]` |
| 历史 reanalyze | ✅ v1 的行全部保留；v2 的用户可见事件与 v1 完全相同（2 次 natural rollover），cycle 边界相同 |
| Workstream B：Claude 数据源 | ✅ 安全路径存在，**不读凭据，不消耗推理额度**。主源：statusLine 桥接（被动）；补充源：`claude -p /usage` 本地命令（主动，默认关闭） |
| Claude 字段 | ✅ 5h / 每周的已用 %、重置时间，额外按模型的周额度（Fable）。availability 永远 Unknown |
| 真实双 provider 测试 | ✅ 2 小时 3 分钟（21:52:34–23:55:54）：Codex 38/38 次轮询成功；Claude 10/10 次 `/usage` 成功，**0 次 429，0 次错误**；Claude 5h 16→28%，每周 31→34% |
| Desktop Code 标签 | ❌ 不运行 statusLine：2 小时内本会话大量使用 Claude，桥接文件一次也没有更新。本机实际数据全部来自主动源（用户已同意开启） |
| Dashboard / 托盘 / 设置 / 历史 / 事件 / 通知 | ✅ 两个 provider 并列；全局 severity 取最大值；暂停作用于全部；通知带 provider 名、按窗口实例去重、跨 provider 合并 |
| 安全 | ✅ 诊断导出扫描：没有 email、orgId、组织名、用户路径、token 值。`~/.claude/settings.json` 只在用户确认预览后修改，停用后与原文件完全一致 |
| 真实测试中发现并修复的缺陷 | **10 个**，其中 1 个严重（诊断导出导致数据库死锁、轮询停止），1 个是 Phase 3 就有的潜在缺陷（启动后第一次读取被推迟）。见 Real dual-provider test |
| 测试 | Rust 135、UI 24、TS 参考实现 83，全部通过；clippy 零警告 |
| 7 天 soak | **可以开始**，条件见 Readiness for 7-day soak |

---

## Claude environment

`[LOCAL TEST]`

| 项目 | 结果 |
|---|---|
| Claude Code CLI | 2.1.278，npm 全局安装。入口 `%APPDATA%\npm\claude.cmd`，二进制 `%APPDATA%\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe`（226 MB） |
| Claude Desktop | 已安装并在运行。它是 **MSIX 打包应用**（`Claude_pzs8sxrjxfjjc`）；本次开发会话就运行在它的 Code 标签中 |
| VS Code Claude 插件 | 未检查 |
| Git Bash | 已安装。Claude Code 在 Windows 上用它执行 statusLine 命令 `[ANTHROPIC DOCS]` |
| 时区 | Korea Standard Time；Claude Code 的 `/usage` 文本标注为 `(Asia/Seoul)` |

### MSIX 文件系统虚拟化（本阶段最重要的环境发现）

`[LOCAL TEST]` 从 Claude Desktop 包内启动的进程（例如 Desktop Code 标签里的 shell，以及它启动的任何程序），写入 `%LOCALAPPDATA%` 时会被 Windows 重定向到：

```text
%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\...
```

`%USERPROFILE%` 下 AppData 之外的路径**不会**被重定向（两个方向都已验证）。

这对 QuotaDock 有三个影响：

1. **数据库**：Phase 3 的真实运行是从这个 shell 启动的，所以 QuotaDock 的数据库实际上写进了包的私有存储。本阶段第一次用 `explorer.exe` 在包外启动 QuotaDock 时，它在真实的 `%LOCALAPPDATA%\QuotaDock` 下看到的是空目录，于是新建了一个空数据库。
2. **桥接文件**：Claude Code 运行在 Desktop 内时，它启动的 statusLine 桥接如果写 `%LOCALAPPDATA%`，也会被重定向，QuotaDock 读不到。所以桥接放在 `~/.quotadock`（见 Selected Claude source）。
3. **开发工具**：开发期间在包内 shell 中运行的任何 QuotaDock 工具，看到的都是虚拟化视图。

数据处理过程（如实记录）：

1. 发现空库后立即强制结束 QuotaDock，避免 WAL checkpoint，并把两边的文件都备份到 scratchpad。
2. **失误**：我随后在包内 shell 里运行了一次 `quotadock-dev summary`。它对虚拟化副本执行了 migration 和 reanalysis。所幸那份副本是完整的原始历史（integrity ok，127 个 snapshot），所以 migration 正是我们想要的结果，没有数据丢失。
3. 用一个在包外运行的批处理，把真实位置的空库移到 `target\realdb-backup`，再把完整历史放进真实位置。
4. 虚拟存储中的 QuotaDock 目录移到 `target\virtual-store-backup\QuotaDock`。
5. 从此以后，QuotaDock 一律通过 `explorer.exe` 在包外启动，数据库只用包外批处理读取（只读）。

正常使用时（在资源管理器中双击、开机自启）QuotaDock 不在任何包内，不受影响。见 Known limitations。

## Claude Code version

- **2.1.278** `[LOCAL TEST]`。
- QuotaDock 每 60 分钟在后台运行一次 `claude --version` 和 `claude auth status --json`（都是本地命令，不调用模型）。
- 版本写入每个 Claude snapshot 的 `source_version`。
- 版本变化不会让 QuotaDock 盲目解析（§72）：
  - `/usage` 每次都要通过外壳检查和严格的文本解析。格式不认识就进入 **"Claude 用量集成需要验证"**，本次运行停用主动刷新，**不猜测**。
  - statusLine 桥接只读取 `rate_limits.<window>.used_percentage/resets_at` 这几个字段。字段缺失时不产生数据，状态变成 "等待" / "非实时"，而不是显示错误的数字。

## Authentication mode

`[LOCAL TEST]` `claude auth status --json` 的非个人字段：

| 字段 | 值 |
|---|---|
| loggedIn | true |
| authMethod | claude.ai |
| apiProvider | firstParty |
| subscriptionType | max |

- 输出中也有 `email`、`orgId`、`orgName`。QuotaDock 只在内存中把 `orgId|email` 哈希成 account fingerprint，然后丢弃原值：`SHA-256("quotadock:claude-account:v1:" + orgId + "|" + email)` 的前 16 字节。
- 检查 API key 覆盖时只看变量**是否存在**，不读取值。检查的变量有 `ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_OAUTH_TOKEN`、`CLAUDE_CODE_USE_BEDROCK` / `_VERTEX` / `_FOUNDRY` 和自定义 `ANTHROPIC_BASE_URL`。读取范围是用户级和系统级注册表环境。本机全部不存在，结论是订阅计费。
- 计费判定：
  - 任何一个覆盖变量存在，或 `authMethod` 不是 `claude.ai`，或 apiProvider 不是 firstParty → **API billing**。
  - UI 显示 "Claude 订阅额度不可用 / 当前 Claude Code 正在使用 API Key 计费"，不显示任何订阅百分比（§64）。
  - 主动刷新也不会运行。
- Claude Desktop 会向包内进程注入大量 `CLAUDE_*` / `ANTHROPIC_*` 变量。QuotaDock 启动 claude 子进程时会清空环境，只从注册表重建用户级和系统级环境。否则测到的是 "Desktop 内嵌会话"，不是用户自己的 Claude Code。

## Officially documented surfaces

`[ANTHROPIC DOCS]`

| 能力 | 文档要点 | QuotaDock 用法 |
|---|---|---|
| statusLine `rate_limits` | `five_hour` / `seven_day` 各有 `used_percentage`（0–100，可带小数）和 `resets_at`（Unix 秒）。只对 Pro/Max 订阅提供；会话中第一次 API 响应之后才出现；每个窗口可能单独缺失 | **主数据源**（被动） |
| statusLine 执行方式 | 事件驱动，可选 `refreshInterval`。Windows 上通过 Git Bash 执行，没有 Git Bash 时用 PowerShell。需要工作区信任。自定义后会隐藏底部大部分快捷键提示 | 桥接命令用正斜杠和 `~` |
| `/usage` | 显示订阅额度和当前限流状态 | `claude -p /usage` 作为可选主动源 |
| `claude auth status` | 登录状态和认证方式 | 判断计费方式 |
| `-p` / `--output-format json` / `--no-session-persistence` / `--model` / `--strict-mcp-config` | CLI 参考 | 主动查询的参数 |

`[CLAUDE CODE OBSERVED]` 本机 `claude.exe` 中能找到 `rate_limits`、`used_percentage`、`resets_at`、`five_hour`、`seven_day`、`spend_limit`、`refreshInterval` 等字符串，与文档一致（只做了字符串扫描，没有反编译）。

**没有使用** `api.anthropic.com/api/oauth/usage`，也没有研究它的请求格式（§10）。

## Data-source candidates tested

| # | 候选 | 结果 | 依据 |
|---|---|---|---|
| 1 | `claude -p /usage --output-format json`，带故意无效的模型名 | ✅ 本地 slash command：`local_command:"usage"`，`duration_api_ms` 0，`num_turns` 0，token 全 0，`modelUsage` 为空，没有残留进程 | `[LOCAL TEST]` `[REAL RUN]` |
| 2 | statusLine 桥接 | ✅ 真实的 Claude Code 2.1.278 交互会话调用了桥接，桥接写出了记录（版本、模型、会话哈希）。真实的 `rate_limits` 数值见 Known limitations | `[LOCAL TEST]` `[ANTHROPIC DOCS]` |
| 3 | `claude auth status --json` | ✅ 计费 / 认证方式 | `[LOCAL TEST]` |
| 4 | Desktop Code 标签是否运行 statusLine | ❌ 在本机上没有观察到（见 Real dual-provider test） | `[REAL RUN]` |
| — | 凭据文件、OAuth 端点、浏览器 cookie、OCR、UI 自动化读取 `/usage` | 未尝试（§9、§10、§19–§21 禁止） | — |

GPT 授权的那一次 "可能消耗少量额度的验证" **没有执行**：零成本方法已经给出结论。

## Selected Claude source

```text
┌ Claude Code（终端 CLI 会话）
│   statusLine → ~/.quotadock/bin/quotadock-claude-bridge.exe [--chain-b64 <原 statusLine>]
│              → ~/.quotadock/claude/statusline.json（原子替换，只含额度字段）
│                                                        │ 每 15 s 读一次（一个小文件）
├ QuotaDock ───────────────────────────────────────────┤
│   ClaudeMonitor
│     passive  : 新记录 → normalize → provider_snapshots（source = statusline）
│     active   : 可选，默认关闭。被动数据过期时运行
│                claude -p /usage --output-format json --model quotadock-usage-guard-no-model
│                       --no-session-persistence --strict-mcp-config
│                → 外壳检查 → 文本解析 → provider_snapshots（source = active_cli）
│     env      : 每 60 min 运行 claude --version / claude auth status --json
└ 优先级：新鲜的 statusLine > 主动查询 > 上一次快照；按时间取最新的有效观测
```

- **被动源（statusLine）**：
  - 用户在 QuotaDock 设置中**显式**点击 "查看改动" → 看到完整的预览（文件、当前 statusLine、新 statusLine、备份路径）→ 点击 "启用"，才会写入 `~/.claude/settings.json`（§18）。写入的正是预览的内容：后端用预览的时间和指纹核对，文件在预览后有任何改动都要求重新预览。
  - 启用前先备份。其他键和键的顺序都保留。已有的 statusLine 不会被覆盖，而是被**串联**：桥接用同样的输入运行原命令，并输出原命令的结果（§17、§71）。
  - 停用时，只有当 statusLine 仍然是 QuotaDock 写入的那个值，才恢复原值；如果用户之后改过，就保持原样（`ChangedByUser`）。
- **主动源（`/usage`）**：
  - 默认关闭，用户在设置中显式打开，界面附有 GPT 指定的说明文字。
  - 运行条件：被动数据在前台超过 5 分钟、在后台超过 15 分钟没有更新。新鲜的 statusLine 数据会让它跳过。
  - 手动刷新最少间隔 60 秒。
- **桥接位置**：`~/.quotadock`（`%USERPROFILE%\.quotadock`），不放在 AppData。原因见 Claude environment 中的 MSIX 部分。只有当前用户能访问（用户目录的默认 ACL），没有命名管道，也没有 HTTP（§69、§70）。

## Why this source was selected

1. **零凭据**：两条路径都由 Claude Code 自己完成认证。QuotaDock 不读 `.credentials.json`，不接触 token，不实现 OAuth（§9）。
2. **零推理**：
   - statusLine 是 Claude Code 在用户自己的会话中推送的。
   - `/usage` 是本地命令。每次运行都检查 JSON 外壳：`local_command == "usage"`、`duration_api_ms == 0`、`num_turns == 0`、token 全为 0、`modelUsage` 为空。任何一项不符就判为 "检测到模型调用"，本次运行永久停用主动源。
   - 另外始终带一个**不存在的模型名**作为保险：如果将来某个版本把 `/usage` 当作 prompt 发给模型，请求会因为模型不存在而失败，不会消耗额度（§65、§66）。
3. **官方文档**：statusLine 的 `rate_limits` 是文档写明的字段；`/usage` 是文档写明的命令（§11、§63）。
4. **结构化程度**：
   - statusLine 是 JSON，带小数百分比和秒级时间戳，所以作为主源。
   - `/usage` 是给人看的文本，只有整数百分比和分钟精度，所以只作为默认关闭的补充。
5. **用户控制**：任何对 `~/.claude` 的修改都需要预览加显式确认，可以恢复（§18、§45、§46）。

## Fields available

见下方 Evidence table。补充说明：

- **剩余百分比**是推导值：`100 − used`，保留原始精度（statusLine 的 `23.5` 得到 `76.5`）。
- **额外额度**（`Current week (Fable)` 这类按模型的周额度）只有 `/usage` 提供，存为 `weekly_model:<name>`，显示在 "其他额度" 中。
- **订阅类型**（`max`）来自 `claude auth status`，显示为 "Claude Max"。

## Fields unavailable

| 字段 | 原因 | QuotaDock 处理 |
|---|---|---|
| availability（allowed/blocked） | 两个来源都没有 | 永远是 Unknown。100% 已用**不**推断为不可用（§31、§59） |
| 限流状态 | `/usage` 只给百分比 | 用量到 100% 时记录一条诊断事件 `limit_reached_observed`（每个窗口实例一次），不改变 availability |
| statusLine 中的按模型额度 | 文档里没有 | 只来自 `/usage` |
| spend_limit | 只在企业网关场景出现，是花费，不是订阅窗口 | 桥接直接丢弃 |
| 秒级重置时间（主动源） | `/usage` 文本只到分钟 | `reset_precision_ms = 60000`；同一实例允许 ±15 分钟误差 |

## Evidence table

§84，本机：

| Field | Claude（statusLine） | Claude（`/usage`） | 依据 |
|---|---|---|---|
| 5h used % | YES（小数） | YES（整数） | `[ANTHROPIC DOCS]` / `[REAL RUN]` |
| 5h remaining % | derived | derived | — |
| 5h reset time | YES（秒） | YES（分钟，本地时区文本） | `[ANTHROPIC DOCS]` / `[REAL RUN]` |
| weekly used % | YES | YES | 同上 |
| weekly remaining % | derived | derived | — |
| weekly reset time | YES | YES | 同上 |
| extra model limits | NO | YES（`Current week (Fable)`） | `[REAL RUN]` |
| availability authority | NO | NO | — |
| subscription plan | NO（来自 `claude auth status`：max） | 只说明 "using your subscription" | `[LOCAL TEST]` |
| source live | conditional：只在 Claude Code **CLI** 会话收到回复后推送；Desktop Code 标签在本机不推送 | YES：打开主动刷新后，按 5 / 15 分钟查询 | `[REAL RUN]` |

## 5h behavior

`[REAL RUN]`，21:55–23:44，10 次观测，全部来自 `/usage`：

```text
used %   16 16 17 18 21 25 25 26 27 28
resets   10-09 01:19 / 01:20（同一个窗口，分钟取整）
```

- 用量随用户正常使用（主要是本次开发会话）单调上升，没有回落，也没有跳变。
- 整个测试期间是同一个窗口实例。重置时间在 01:19 和 01:20 之间来回，差 60 s，这是 `/usage` 文本的分钟取整（±15 分钟的同一实例容差覆盖了这一点）。没有产生 `quota_window_changed` 或刷新事件。
- 测试期间没有经过 5h 重置（01:20 在测试结束之后），所以没有观察到 Claude 的 5h 刷新。之后 QuotaDock 继续运行，可以在 soak 中观察。
- 测试后（00:11–00:16）观测到 31%，窗口仍是同一个。
- 5h 提醒默认关闭；托盘只显示数值。预测：5h 用 30 分钟的最低观察要求，最终版本显示 "低"，约 6.6 pp/h。

## Weekly behavior

`[REAL RUN]`：

```text
weekly (all models)   31 31 31 32 33 34 34 34 34 34
weekly (Fable)        1 × 10
resets                10-12 23:59 / 10-13 00:00（同上，分钟取整）
```

- 同一个窗口实例。Claude Code 文本里同一个时刻有时显示 `Oct 12, 11:59pm`，有时显示 `Oct 13, 12:00am`。
- 每周提醒默认开启（30% / 10% 剩余）。测试期间剩余 66–69%，没有触发。
- **预测缺陷（已修复）**：
  - 测试结束时，旧版本把这两个小时晚间会话的速度（31→34%，约 43 pp/天）外推到本周剩余的 3 天多，显示 "可能提前耗尽"，并把托盘提高到 Warning。
  - 但本周窗口从 10-06 00:00 开始，到这时平均只有约 11 pp/天（34% / 3 天），按这个速度到重置时约 77%，不存在风险。
  - 修复 `[QUOTADOCK HEURISTIC]`：Claude 的每周预测需要窗口实例内至少 **24 小时**的观察才计算（§33 "有足够历史后"）；不足时显示 "历史数据不足 · 至少需要 24 小时的观察和可测量的用量"，不参与托盘 severity。这个时长由后端随预测一起下发（`requiredSpanMs`），前端不写死阈值。5h 仍是 30 分钟。
  - 回归测试 `a_weekly_claude_forecast_needs_a_day_of_history` 使用这次的真实数字。

## Additional buckets

- `/usage` 报告了 `Current week (Fable): 1% used`，与 all-models 周额度同一个重置时间 `[REAL RUN]`。
- QuotaDock 把它存为 `weekly_model:fable`（kind = `extra`，label = `Fable`），在 Claude 卡片的 "其他额度" 中显示为 "Fable（每周） 99% 剩余"。
- 额外额度**不参与**托盘 severity，也不触发通知。它们的语义（哪些模型、是否和总额度叠加）`[UNKNOWN]`。
- statusLine 中出现的未知窗口键会原样保存为 `statusline:<key>`，不会被重新解释。

## Reset timestamps

- **statusLine**：`resets_at` 是 Unix 秒，按原值保存，精度 1000 ms `[ANTHROPIC DOCS]`。
- **`/usage`**：
  - 文本如 `resets Oct 9, 1:19am (Asia/Seoul)`。QuotaDock 按**本地时区**解析，选与当前时间最接近的年份，并限制在 [now − 2 天, now + 9 天] 之内，精度记为 60000 ms。
  - 时区标签只记入诊断，不参与换算。本机标签（Asia/Seoul）与 Windows 时区（KST）一致。
- **同一实例的判定** `[QUOTADOCK HEURISTIC]`：
  - 新的 resetsAt 与已打开实例的相差 ≤ 15 分钟 → 同一实例。
  - resetsAt 单独变化**不**作为重置证据（§60）。
  - 只有当上一个重置时间已经过去，并且出现了更晚的重置时间，才会产生用户可见的 `quota_refreshed_observed`。
  - 其他移动只记诊断事件 `quota_window_changed`。
- **真实稳定性** `[REAL RUN]`：10 次观测中，每个窗口的 resetsAt 只有 2 个不同的值，相差正好 60 s（分钟取整）。没有新实例，没有 Claude 事件。statusLine 的秒级时间戳在本机还没有观测到（见 Known limitations）。

## Polling / push behavior

| 部分 | 节奏 | 说明 |
|---|---|---|
| 被动桥接 | 每 15 s 读一次 `statusline.json` | 只比较 `receivedAtMs`，没有新记录就不做任何事。暂停期间到达的记录不会入库 |
| 主动 `/usage` | 前台 ≥ 5 min，后台 ≥ 15 min | 只在最新的 Claude 数据（不论来源）已经超过这个间隔时运行。不强行套用 Codex 的 60 s / 5 min（§62） |
| 手动刷新 | 立即刷新所有可刷新的 provider | Claude 只有在 `refreshCapability = active` 时才会运行 `/usage`，最少间隔 60 s。被动模式下刷新**不会**为了数据启动 Claude 或发送 prompt（§78） |
| 环境检查 | 启动时一次，之后每 60 min | `--version` 和 `auth status` |
| 新鲜度 `[QUOTADOCK HEURISTIC]` | 只有被动源：≤ 10 min 为 Live（实时）。主动源可用时：≤ 20 min（后台间隔 15 min + 5 min） | 超过后显示 Stale（"上次更新：X 前"）。从未收到数据：Waiting（"等待 Claude Code"）。不会伪造实时状态（§82）。严重级别只看额度阈值，不随新鲜度变化，所以托盘图标不会因为数据变旧而闪烁 |

`refreshCapability`（§79）：`active`（主动源可用）/ `passive`（只有桥接）/ `unavailable`（两者都没有）。UI 用它决定刷新按钮的行为。

单次主动查询的子进程：

- 干净环境、`CREATE_NO_WINDOW`、Job Object（QuotaDock 退出时一并结束）、45 s 超时。
- 固定工作目录 `%LOCALAPPDATA%\QuotaDock\claude\cwd`。
- `--no-session-persistence`（不写会话文件）、`--strict-mcp-config`（不加载 MCP 服务器）。

## Rate-limit / 429 behavior

实现（GPT 的决定，`[QUOTADOCK HEURISTIC]`）：

1. `/usage` 返回 429 时，优先遵守 `Retry-After`（有效范围 1 s–24 h）；没有时按 15 → 30 → 60 分钟退避。
2. 连续 3 次 429 → 本次运行中 "Claude 主动刷新已暂停"。
   - **不会**把整个 Claude provider 标成错误。
   - 被动桥接继续工作。
   - UI 显示 "主动刷新暂时受限" / "Active refresh temporarily limited"。
3. 其他来源错误（超时、退出码非 0）按 15 / 30 / 60 分钟退避，不会暂停。
4. 外壳显示模型调用，或格式无法识别 → 本次运行停用主动源，显示 "需要验证"。

测试：`rate_limits_back_off_and_suspend_the_active_source_after_three`（使用伪造的 claude 可执行文件）。

**真实运行** `[REAL RUN]`：

- 测试期间 10 次主动查询，测试后又有 2 次，共 12 次，**0 次 429**，0 次错误，0 次外壳异常。
- 耗时 2.3–8.7 s，中位数约 2.7 s。8.7 s 那次与一次 release 编译同时发生；6.5 s 那次原因未确认（可能是 15 分钟间隔后的冷启动）。
- 节奏：前台 302 / 303 s，后台 903–909 s。有一次 +528 s，是用户在 23:37 左右打开了窗口（变成前台节奏），之后又最小化。
- 规格 §10 提到社区报告过 429 和多会话轮询饱和 `[CLAUDE CODE ISSUE]`。在本测试的频率（每 5 / 15 分钟一次）下没有出现 429。2 小时的样本不足以证明长期安全，soak 中会继续统计。

## Security

| 规则 | 结果 |
|---|---|
| 不读取 `~/.claude/.credentials.json`，不复制 token / cookie，不实现 OAuth | ✅ 代码中没有访问这些路径的语句；Claude 子进程自己完成认证 |
| 不调用 undocumented API，不请求 `api/oauth/usage` | ✅ QuotaDock 本身不发出任何 HTTP 请求 |
| 不读取浏览器 cookie；没有 OCR、截图或 UI 自动化读取 `/usage` | ✅ |
| 默认不写 `~/.claude/settings.json` | ✅ 只有显式启用时才写：先预览，再备份，保留其他键，串联已有 statusLine，可以恢复 |
| 预览就是实际写入的内容 | ✅ 本阶段真实测试中修复：启用时用预览的时间和指纹重新生成计划，任何不一致都拒绝（见 Real dual-provider test 中的缺陷 4） |
| 监控本身不发送 prompt、不启动 agent、不调用模型 | ✅ 每次主动查询都做外壳检查，并带无效模型名作为保险 |
| 桥接 tiny / fast / non-blocking | ✅ 2.0 MB 单文件 exe。stdin → 过滤 → 原子写 → 输出一行。没有 SQLite、没有网络、不启动 QuotaDock。实测热启动中位数 15 ms，冷启动 386 ms。串联命令有 5 s 超时 |
| IPC | ✅ 原子替换的本地文件，位于当前用户的用户目录下。没有 localhost HTTP，没有全局可写的 pipe |
| webview 权限 | ✅ 18 个类型化命令。新增的 3 个 Claude 集成命令都不接受路径或内容参数（启用命令只接受预览的时间和指纹，由后端验证） |
| 不安装或修改 Claude Code 本身 | ✅ |
| 不遥测、不自动更新 | ✅ |

## Credential handling

- 只检测**认证方式**：通过 `claude auth status --json` 的 `loggedIn`、`authMethod`、`apiProvider`、`subscriptionType`。
- 只检测环境变量是否**存在**，不读取值。
- `email` / `orgId` 只在内存中参与哈希，从不写盘、显示或导出。
- Claude Desktop 注入的会话变量（消息 socket、token 等）在启动子进程前全部清除，不会传给 claude.exe，也不会记录。
- 保存的原始数据是**清洗后**的（§56）：
  - 桥接记录：额度字段、Claude Code 版本、模型显示名、会话 ID 的 8 位 SHA-256 前缀。
  - `/usage` 记录：只有 `Current …` 行和 "using your subscription" 那一行，并经过 `sanitize_string` 处理。
  - 不保存提示词、路径、cwd、transcript 路径、email、组织名或任何凭据。
- 诊断导出中的 Claude 部分只有版本、安装类型、计费方式、认证方式、订阅类型、覆盖变量是否存在、主动源状态和运行记录。
- **真实导出扫描** `[REAL RUN]`（`diagnostics-20261009-001208.json`，205 KB）：
  - Claude 账号的 email、orgId、组织名：都不存在（与 `claude auth status` 的实际值逐一比对，比对值只在内存中使用）。
  - 用户 email、Windows 用户名或用户目录路径、`@`、`Bearer`、`Authorization`、`cookie`、`sk-ant`、`credentials`、`access_token`、`refresh_token`、`session_id`、`transcript`、`projects`：0 次。
  - `token` 出现 2 次，都是字段名 `authToken: false` / `oauthToken: false`（只表示覆盖变量不存在）。

## Provider-neutral database migration

| 版本 | 名称 | 内容 |
|---|---|---|
| v4 | `phase35_version_free_dedup_scopes` | 通知去重的 scope key 去掉 classifier 版本前缀（`reset-detector-v1\|…` → `…`），classifier 升级后不会重复提醒 |
| v5 | `phase35_provider_neutral` | 新建 `provider_snapshots`、`provider_windows`、`provider_window_instances`、`provider_events`、`provider_source_runs` |

与 §53 推荐模型的差异：

- 没有单独的 `provider_extra_limits` 表。额外额度是 `provider_windows` 中 `kind = 'extra'` 的行，带 `label`。
- Codex 的 Phase 2 表**不迁移**，因为 Codex 的 reset 引擎依赖那些列（§54）。
- **读取层**是 provider-neutral 的：
  - `EventView.provider`：Codex 和 Claude 的事件在同一条时间线里，按 `observed_at_ms` 做 keyset 分页。
  - `HistorySeries.provider` / `HistoryMarker.provider`。
  - Dashboard / 托盘的 provider 列表。

迁移前后 Codex 数据 `[REAL RUN]`：

| 项目 | 迁移前（v3） | 迁移后（v5） |
|---|---|---|
| snapshots | 127 | 127（之后继续增长） |
| reset-detector-v1 events | 9 | 9（原样保留） |
| reset-detector-v1 cycles | 4 | 4（原样保留） |
| notification_log / notification_state | 0 / 0 | 0 / 0 |
| integrity_check | ok | ok |

v4 的 scope 重写在单元测试中覆盖（`a_new_classifier_reanalyses_history_and_keeps_earlier_rows` 等）。真实库没有去重行，所以这一步是 no-op。

## Dashboard integration

`[REAL RUN]` 截图 `real35-overview.png`、`real35-overview-2205.png`（只截 QuotaDock 自己的窗口）：

- **Overview**：
  - 每个 provider 一行：左边是额度卡片，右边是预测卡片，保持 Phase 3 的视觉风格，没有重做 UI（§75）。
  - Claude 卡片显示：计划名（Claude Max）、新鲜度徽标（"实时" 或 "X 前更新"）、5h 和每周剩余、已用、重置时间、"其他额度"。非正常状态（等待 Claude Code / 未设置 / API 计费 / 未登录 / 未安装 / 需要验证 / 数据源出错）显示为卡片上的横幅；关闭 Claude 监控时整张卡片隐藏。
  - 可用性始终显示为 Unknown，附说明 "Claude 不提供可用状态。QuotaDock 不会根据百分比推断"。
  - Claude 预测卡片：5h 需要至少 30 分钟、每周需要至少 24 小时的观察和可测量的用量，否则显示 "历史数据不足"，并写明所需时长。
- **History**：增加 provider 选择器（全部 / Codex / Claude，默认全部，§47）。Claude 的序列在窗口实例之间断开，不连线。
- **Events**：同一时间线显示两个 provider 的事件，每条带 provider 标签。
- **Settings**：
  - "AI 服务" 区：Codex（已连接，数据来源 Codex app-server）和 Claude（启用开关、状态栏桥接状态、查看改动 / 停用并恢复、主动刷新开关及说明）。关闭 Claude 监控时，如果桥接还在，会先询问 "关闭并恢复"（§46）。
  - "通知" 区：新增 Claude 组。
  - "监控" 区：说明 Claude 的节奏。
- **数据来源标识**（§73）：Claude 卡片右上角是新鲜度徽标（"实时" 或 "X 前更新"）。旁边的提示写明 "数据来源: Claude Code 状态栏 / Claude Code 主动刷新"。Overview 本身不强调内部实现。
- **产品名**：QuotaDock，侧栏中显示副标题 "AI 用量管理 / AI Usage Manager"（§74）。字符串在开发中就已存在，但收尾检查时发现没有被渲染，已补上。

UI 状态矩阵（§58），在浏览器中用 mock 后端渲染检查：

- `?claude=` 参数：live、stale、waiting、notconfigured、apibilling、notfound、low、off（off 时 Claude 卡片整个隐藏）。
- 组合场景：`?scenario=error&claude=live`（Codex 出错 + Claude 正常）、`?scenario=low&claude=low`（两者都偏低，严重横幅同时列出 Codex 和 Claude 的原因）。
- not_authenticated、source_error、needs_verification 的文案由 Rust 的状态映射和 i18n 键覆盖，没有单独的 mock 场景。

## Tray integration

- **Tooltip**（`[REAL RUN]`，通过 UI Automation 从托盘溢出区读取）：

  ```text
  QuotaDock
  Codex 5小时：100% · 每周：92%
  Claude 5小时：83% · 每周：69%
  刚刚更新
  ```

- **菜单**：

  ```text
  打开 QuotaDock / 立即刷新
  Codex / 5 小时 … / 每周 …
  Claude · <状态> / 5 小时 … / 每周 …
  暂停监控 / 退出
  ```

  Claude 关闭监控时，Claude 行显示 "Claude · 已关闭"。
- **全局 severity** = 各 provider 的最大值（§41、§42）。
  - Claude 的 severity 只来自额度阈值，并且只看**当前窗口实例**（重置时间还没过）的数据，规则开关与提醒一致（每周默认开，5h 默认关）。
  - 数据变旧时仍然计入：同一窗口内用量只增不减，旧值是用量的下限。所以 "非实时" 不会让托盘降级，也不会因为刷新节奏而闪烁。
  - 已过重置时间的窗口、没有数据（未设置 / 等待 / API 计费）不会提高 severity。
  - Claude 自身的状态（非实时、等待、数据源错误）**不会**把托盘变成 Error，因为 Claude 监控是可选的；只在 Claude 行后面加注，例如 "· 非实时"。一个 provider 出问题也不会让另一个 provider 的行消失。
  - 测试：`tray_takes_the_highest_severity_and_keeps_every_provider`、`claude_tray_line_counts_only_current_enabled_windows`。
- **暂停**作用于所有 provider（§43）：
  - Codex 停止轮询，app-server 退出。
  - Claude 不运行主动查询；暂停期间到达的桥接记录不入库。
  - 恢复后两者立即刷新。

## Notifications

| 类别 | 默认 | 说明 |
|---|---|---|
| Claude 每周额度提醒 | **开** | 剩余 30% / 10% 各一次（每个窗口实例） |
| Claude 5 小时额度提醒 | 关 | 同上 |
| Claude 预测风险（每周） | 关 | 被动数据可能较稀疏 |
| Claude 额度刷新通知 | 关 | 事件本身是用户可见的，只是默认不弹通知 |

- **标题带 provider 名**：例如 "Claude 每周额度低于 10%"；同一 provider 多条合并时是 "Claude 额度较低"。
- **去重**按 provider + 窗口实例持久化。重启后不会重复提醒；新的实例会重新提醒。
  - 测试 `claude_weekly_threshold_dedup_survives_restart_and_rearms_after_refresh` 覆盖 §58 的序列：31 → 29 产生一条 30% 提醒；29 → 9 产生一条 10% 提醒；重启后仍是 9，不重复。
- **跨 provider 聚合**（§51）：同一次评估中如果 Codex 和 Claude 都偏低，合并成一条 "AI 额度偏低 / AI quotas are running low"，正文分别列出（例如 "Codex 每周：剩余 8%"、"Claude 每周：剩余 9%"）。测试 `codex_and_claude_low_in_one_evaluation_become_one_notification`。
- 已过重置时间的窗口不会驱动提醒（`a_passed_reset_never_drives_reminders`）。
- **真实运行**：0 条通知。Codex 每周剩余 90–92%、Claude 每周剩余 66–69%，都没有达到 30% 阈值；Claude 预测提醒默认关闭。没有为了触发通知去消耗额度。

## Codex detector-v2 fix

**问题**（Phase 3 真实数据，2026-10-08）：

- 5h 窗口空闲约 9 小时。空闲期间 Codex 每次都报告 `resetsAt = now + 5h`（sliding）。
- 之后用户开始使用。reset-detector-v1 把这段空闲当作同一个分析段的一部分，于是 burn rate 被约 9 小时的 0% 样本稀释，预测严重偏低。

**修复**（`reset-detector-v2`）：

1. 检测到 sliding 时，窗口状态记为 `awaiting_usage`。
2. 在同一个 ObservedCycle 内，第一次出现 used > 0 时，如果满足以下全部条件，就把**分析段**重新锚定到这一次观测：
   - 参考快照 used ≤ 0；
   - 参考快照的重置时间还没有过；
   - 派生的起点落在 [cur − 5h − 容差, cur + 容差] 之内。

   这一步**不**开新 cycle，**不**产生事件，只写一行日志 `analysis segment re-anchored window=five_hour … derived`。
3. 派生起点 `nominalActiveWindowStart = resetsAt − 300 min` 存为 `active_origin_ms`，在模型和日志中都标注为 **derived, not authoritative**。
4. burn rate 对重新锚定的段使用**过原点的最小二乘**（原点 = 派生起点，0%）。空闲快照不进入分母。

**回归测试** `idle_to_active_reanchors_analysis_segment`，使用真实 fixture `tests/fixtures/real-5h-idle-to-active-2026-10-08.json`（snapshot #13–#107，只含规范化后的数字）：

| 指标 | v1 方式（从空闲起点算） | v2 |
|---|---|---|
| 分析段起点 | #13（空闲开始） | 第一次有用量的快照 |
| burn rate | 2.24 pp/h | **12.9 pp/h** |
| 参考：派生起点到最后一次观测的平均速度 | — | ≈ 9.3 pp/h |

- 没有假的 rollover，没有 UnscheduledQuotaRefresh，没有 WindowScheduleChanged，没有用户可见事件，5h 的 cycle close 次数为 0。
- `tests/detector_v2.rs` 另有 5 个用例：
  - `first_active_poll_with_a_new_reset_is_a_resumption_not_a_schedule_change`；
  - `frozen_zero_percent_anchor_then_usage_reanchors_at_the_usage`；
  - `a_window_that_was_never_idle_is_not_reanchored`；
  - `a_rollover_after_reanchoring_clears_the_derived_origin`；
  - `version_free_dedup_scopes`。

## Historical reanalysis

- `CLASSIFIER_VERSION = "reset-detector-v2"`。派生表（events、cycles、candidates、window_tracks、engine_state）都以 classifier 版本为键。
- 启动时如果当前版本没有 `engine_state` 而 snapshot 数大于 0，就自动 `reanalyze_all()`，并写一行日志。
- **v1 的行原样保留**，便于对比和回滚。

真实库对比 `[REAL RUN]`，截至 v1 最后处理的 snapshot #127：

| 事件类型 | v1 | v2 |
|---|---|---|
| natural_rollover_observed（user，5h） | 2 | 2 |
| sliding_anchor_detected（diagnostic） | 2 | 2 |
| reset_credit_count_changed（diagnostic） | 1 | 1 |
| transient_backend_state（diagnostic） | 4 | 4（之后的真实运行中又新增了 4 条 resetsAt ±1 s 抖动，都是诊断事件） |
| cycles | 4 | 4（边界相同） |

真实用户可见事件没有被破坏，也没有新增（§80 C）。

- 通知去重 scope 不再带版本前缀（migration v4），所以升级 classifier 不会让已经发过的提醒重新发送。
- 测试 `a_new_classifier_reanalyses_history_and_keeps_earlier_rows`。

## Real dual-provider test

### 设置

- release exe，通过 `explorer.exe` 在 MSIX 包外启动（21:52:34），连接本机真实的 Codex（ChatGPT Plus）和 Claude（Max）账号。
- 21:54 在 QuotaDock 设置中通过预览对话框启用 statusLine 桥接，并打开主动刷新（用户的选择："桥接 + 主动刷新"）。
- 窗口状态：
  - 21:52–22:06 前台；
  - 22:06 关到托盘（后台）；
  - 23:37 左右用户自己打开过窗口，之后最小化；
  - 23:55 我恢复窗口开始收尾检查。
- 用户正常使用：
  - Codex：5h 从空闲的 0% 用到 13%。
  - Claude：主要是本次 Desktop 开发会话，5h 从 16% 到 28%。
  - 没有为测试人工消耗任何额度。
- 测试窗口：21:52:34–23:55:54，**123.3 分钟**。

### §81 记录项

| 记录项 | 结果 `[REAL RUN]` |
|---|---|
| Claude source updates | 10 次，全部来自 `active_cli`；statusLine **0** 次（桥接文件最后一次更新是 21:57:06 的 CLI 实测） |
| 5h utilization | 16 → 28%（一个窗口实例） |
| weekly utilization | 31 → 34%；Fable 1% |
| resetsAt stability | 每个窗口只有 2 个不同值，相差 60 s（分钟取整）；没有新实例，没有 Claude 事件 |
| source stale periods | 两次更新之间最长 15.2 min。旧版本在后台每 15 分钟中有约 5 分钟显示 "非实时"（6 段，共 30.5 min），已修复（缺陷 6）；按新规则为 0 |
| 429 count | **0**（测试后 2 次也是 0） |
| source errors | **0** |
| Codex regression | 38/38 次轮询成功；见下表 |

### Codex 回归

| 项目 | 结果 |
|---|---|
| 轮询 | 38 次，全部成功：1 次 startup、37 次 scheduled |
| 节奏 | 前台 13 次间隔 60–61 s；后台 20 次间隔 300–302 s；另有 4 次 96–225 s，由窗口显示 / 最小化引起 |
| 单次耗时 | 中位数 530 ms，最长 1523 ms |
| app-server | 1 次启动，没有意外退出；QuotaDock 退出（包括强制结束）后 codex.exe 一并结束；Codex Desktop 自己的 codex.exe 不受影响 |
| 事件 | 4 条诊断 `transient_backend_state`（weekly 两次、5h 两次，都是 resetsAt ±1 s 的 A→B→A 抖动）；**0 条用户可见事件** |
| 资源 | quotadock-app 工作集 36.6 MB，2 小时 CPU 7.6 s；QuotaDock 的 codex app-server 49.7 MB，4.6 s。claude.exe 只在 `/usage` 期间存在 2–9 s |

### detector-v2 在真实运行中的重锚

```text
#148 22:40:50  5h  0%  resetsAt 03:40:51 （sliding：每次都是 now + 5h）
#149 22:45:50  5h  6%  resetsAt 03:44:03  ← 用户开始使用 Codex
#150 22:50:51  5h 12%  resetsAt 03:44:03
#151 22:55:52  5h 13%  resetsAt 03:44:03
#152 23:00:52  5h 13%  resetsAt 03:44:04  suspect_transient → transient_backend_state（诊断）
… 13% 保持到 23:55
```

- `analysis_segment_start_snapshot_id` 从 108 变为 **149**；`active_origin_ms` = 22:44:03（= 03:44:03 − 5h，derived）。
- cycle 8（#108 开始）保持打开，**没有新 cycle，没有事件**。
- 稳定观测之后，`confirmed_reset_at_ms` = 03:44:03。
- 这正是 Phase 3 暴露的问题场景。v1 会把 #108 以来约 3 小时的 0% 快照计入分母。

**发现的局限（未修改）**：

- 这次的用量是前置的：11 分钟内从 0% 到 13%，之后 70 多分钟没有变化。过原点最小二乘给出 12.2 pp/h（00:12）和 10.3 pp/h（00:20）。
- 对比：派生起点以来的平均速度约 8.8 pp/h，最近的速度是 0。
- 风险判断仍然正确（"低"：按这个速度耗尽也在 03:44 重置之后）。
- 但数值偏高。用量先快后慢时，这种拟合会系统性偏高；先慢后快时会偏低。规格 §6 只要求空闲快照不进入分母，没有规定拟合方法。所以本阶段只报告，建议在 Phase 4 由 GPT 决定是否改用 "派生起点以来的平均速度"。

### 收尾检查

1. **诊断导出 → 发现严重缺陷**：
   - 23:56 在旧版本中点击 "导出诊断信息" 后没有任何反馈。
   - 之后 Codex 轮询停止，最后一次是 23:55:54。原因见缺陷 1。
   - 测试窗口在这之前已经满 2 小时，测试数据不受影响。
   - 00:10 强制结束旧进程：它的 codex.exe 由 Job Object 一并结束，没有孤儿进程。
2. **新版本导出**：成功生成文件，扫描结果见 Credential handling。
3. **停用并恢复**（真实 UI）：
   - `settings.json` 恢复为原来的 4 个键，没有 statusLine。
   - 与启用前的备份**逐值相同，键顺序相同**。
   - `~/.quotadock/bin` 中的桥接被删除。
   - 界面显示 "已恢复原来的状态栏。"
4. **重新启用**（新的预览流程）：
   - 对话框显示的备份文件是 `claude-settings.backup-1791472381290.json`。
   - 等 8 秒后点击 "启用"，实际写出的正是这个文件名。
   - statusLine 写回；新版本的桥接已安装。
   - 已安装的桥接通过 Git Bash 管道和 PowerShell 管道都能正确解析（使用临时 USERPROFILE，不污染真实数据）。
5. **正常退出**：
   - 通过设置页 "退出 QuotaDock"，3.3 s 后进程退出。
   - QuotaDock 的 codex.exe 一并退出，没有遗留的 claude.exe。
   - Codex Desktop 的 codex.exe（46360 / 58228）不受影响。
6. **最终版本**（收尾检查中又修复了缺陷 9、10，因此重新编译并重启了几次；每次启动都只读取一次，退出后都没有遗留进程）：
   - 已重新启动（托盘中运行），保持用户选择的配置：桥接 + 主动刷新。
   - 每周预测显示 "历史数据不足 · 至少需要 24 小时"，没有误报横幅。

### 真实测试中发现并修复的缺陷

| # | 缺陷 | 影响 | 修复 | 回归测试 |
|---|---|---|---|---|
| 1 | **诊断导出死锁**（严重）：导出的 Claude 部分在 `with_db` 闭包内又调用了 `with_db`，而数据库 mutex 不可重入 | 导出卡住，之后所有轮询和界面读取都被阻塞，监控静默停止 | 先读取集成状态再进入闭包；`with_db` / `with_db_mut` 增加线程内重入检测：嵌套调用返回错误而不是死锁（debug 构建直接断言） | `diagnostics_export_includes_claude_without_account_values`、`nested_database_access_fails_instead_of_deadlocking` |
| 2 | 导出失败时界面没有任何提示 | 用户不知道失败 | 显示 "导出失败：…" | 类型检查 |
| 3 | Claude 每周预测用 2 小时数据外推整周 | 误报 "可能提前耗尽"，托盘变成 Warning | 每周预测需要 24 小时观察；所需时长由后端下发 | `a_weekly_claude_forecast_needs_a_day_of_history`、UI `forecast history requirement` |
| 4 | 启用预览不精确：备份文件名与预览不同；"预览已过期" 检查实际上是拿计划和它自己比较 | 违反 "show exactly what will change" | UI 回传预览的时间和指纹（文件内容哈希 + 两个 statusLine + 路径）；后端按预览时间重新生成计划，不一致就拒绝；settings.json 的任何改动都要求重新预览 | `enabling_applies_exactly_the_reviewed_plan` |
| 5 | 关闭 "Claude 监控" 时，桥接留在 Claude Code 设置中，而且 "停用并恢复" 按钮被隐藏（§46） | 留下 wrapper，用户无法恢复 | 关闭时弹出 "关闭 Claude 监控？… 关闭并恢复"；只要桥接还在，设置中就一直显示恢复按钮和提示 | mock UI 中实测 |
| 6 | 后台主动刷新间隔 15 分钟，但 "实时" 只认 10 分钟 | 每 15 分钟中有 5 分钟显示 "非实时"，误导用户 | 主动源可用时，实时窗口为 20 分钟 | `data_on_the_active_schedule_stays_live_between_background_refreshes` |
| 7 | 桥接的 stdin 带 UTF-8 BOM 时解析失败（Windows PowerShell 5.1 管道会加 BOM） | 用户用 PowerShell 包装 statusLine 时收不到数据 | 去掉开头的 BOM | `stdin_with_a_utf8_bom_is_parsed` |
| 8 | soak 报告在 v1 / v2 共存后重复统计事件，也没有 Claude 部分 | soak 数据错误 | 只统计当前 classifier；新增 Claude 部分：数据来源、运行状态和 429、最长间隔、实例、resetsAt 分布、事件 | 在真实库上运行 |
| 9 | **启动后的第一次读取被推迟**（Phase 3 就有的潜在缺陷）：第一次读取之前，窗口显示 / 隐藏会把截止时间设为 "现在 + 间隔"，而 `tokio::select!` 在两者同时就绪时随机选择 | 有时启动后 60 s 甚至 5 min 才第一次读取 Codex。真实证据：00:28:59 启动的实例到 00:30:01 才读取；00:26:45 启动后立即隐藏的实例超过 1.5 min 没有读取 | 第一次读取之前不再改动截止时间 | `window_changes_before_the_first_read_do_not_delay_it`（已确认在旧逻辑下失败）；修复后实测启动 2–2.5 s 内读取 |
| 10 | 副标题 "AI 用量管理" 的字符串存在但没有渲染（§74） | 产品名要求未满足 | 在侧栏中显示 | 真实窗口截图 |

缺陷 1–3 是在真实运行中**用户实际会遇到**的问题，自动化测试之前没有覆盖。缺陷 1 尤其严重：它是本阶段新增的代码引入的，Phase 3 的导出没有这个问题。

另外观察到但**没有完全解决**的一点：在启动后约 0.8 s 内（主窗口还在创建、尚未显示时）向窗口发送 WM_CLOSE，进程会退出。原因是 Tauri 2.12.1 在调用我们的 setup 之前就创建配置中的窗口（`app.rs` 2691–2697 行），这时的关闭消息不经过我们的处理。用户无法触发：窗口在 setup 完成前是隐藏的。只有自动化脚本才会这样做，本次就是我的 UI 自动化触发的。已在窗口事件和单实例回调中加入防护：AppState 还不存在时不再 panic。是否需要进一步处理留给 Phase 4。

## Known limitations

1. **Claude Desktop 的 Code 标签不运行 statusLine** `[REAL RUN]`：
   - 本次开发会话就运行在 Desktop Code 标签中，测试期间产生了大量模型调用，但桥接文件一次也没有更新（最后一次更新是 21:57 的 CLI 实测）。
   - 所以对于主要通过 Desktop 使用 Claude 的用户，被动源不会产生数据。真正让数据保持新鲜的是可选的主动源。
   - 如果不开主动刷新，卡片会如实显示 "等待 Claude Code" 或 "上次更新：X 前"（§82），不会伪造实时状态。
   - VS Code 插件是否运行 statusLine `[UNKNOWN]`。
2. **本机还没有观察到真实的 statusLine `rate_limits` 数值**：
   - 测试期间没有在终端的 Claude Code 中发送消息（不为测试人工消耗额度）。
   - 桥接已经用真实的 Claude Code 2.1.278 进程验证过（记录写出，版本、模型、会话哈希都正确），也用文档格式的输入验证过（单元测试、Git Bash 管道和 PowerShell 管道的手工测试）。
   - 用户第一次在终端里使用 Claude Code 并收到回复时，snapshot 的 source 会变成 `statusline`，可以在 Diagnostics 或 soak 报告中确认。
3. **`/usage` 是给人看的文本**：
   - 解析依赖英文格式 `Current session: N% used · resets <Mon D, h:mma> (<TZ>)`；百分比是整数，时间到分钟。
   - 时间按 Windows 本地时区解析，括号中的时区标签只记入诊断。如果 Claude Code 显示的时区与 Windows 不同，重置时间会偏差，目前没有处理。
   - 格式变化时进入 "需要验证"，不会猜测。
   - 重置时间在 `11:59pm` 和 `12:00am` 之间来回变化（见 Weekly behavior），这是 Claude Code 显示时的分钟取整，不是窗口变化。
4. **`/usage` 背后可能有网络请求** `[INFERENCE]`：
   - 很可能是 Claude Code 用它自己的凭据向 Anthropic 查询用量，所以可能被服务端限流（`[CLAUDE CODE ISSUE]` 有 429 报告）。
   - 主动源因此默认关闭，并有退避和暂停机制。本次 12 次查询中 0 次 429。
5. **主动刷新的副作用**：
   - 每次运行启动一次 claude.exe，约 2–7 s（冷启动更慢）。
   - Claude Code 会为 QuotaDock 的固定工作目录创建一个空目录 `~/.claude/projects/C--Users-<user>-AppData-Local-QuotaDock-claude-cwd/memory`。只创建一次，不会增长。这是 Claude Code 自己的行为，不是 QuotaDock 写入的。
6. **statusLine 的副作用**：
   - 自定义 statusLine 会替代 Claude Code 底部大部分快捷键提示（文档行为，启用对话框中已经说明）。
   - statusLine 只在**受信任的工作区**运行。在没有接受信任提示的目录（例如用户主目录）中不会更新。
7. **MSIX 虚拟化**：
   - 如果 QuotaDock 从 Claude Desktop 包内的进程启动（例如在 Desktop Code 标签的终端里直接运行 exe），它的 `%LOCALAPPDATA%` 会被重定向到 Claude 包的私有存储，看到的是另一份数据库。
   - 在资源管理器中双击或开机自启都不受影响（目前还没有安装程序和开始菜单项，见 Phase 4）。
   - 建议 Phase 4 在启动时检测 package identity，并通过 `explorer.exe` 重新启动自己（或给出提示）。
8. **Claude 预测**：
   - 5h 需要至少 30 分钟的观察和可测量的用量；每周需要至少 24 小时（见 Weekly behavior）。所以开始监控后的第一天，每周预测会显示 "历史数据不足"。
   - `/usage` 只有整数百分比，所以 burn rate 很粗。
   - 预测通知默认关闭。
9. **Claude 的 reset 只做实例跟踪**（§61）：
   - 没有 Codex 那样的 reset credit / unscheduled refresh 分类。
   - 只有 "上一个重置时间已过并出现更晚的重置时间" 才算额度刷新。
   - 本次测试期间没有发生 Claude 的重置。
10. **detector-v2 的过原点回归是启发式**：
    - 它假设 5h 窗口正好从 `resetsAt − 5h` 开始，并且之后大致线性消耗。
    - 用量先快后慢时偏高：真实运行中给出 12.2 pp/h，而平均速度约 8.8 pp/h（见 Codex 回归）。先慢后快时偏低。
    - 派生起点只用于 burn rate，不作为 reset 证据，也不会显示成 "窗口开始时间"。
11. **真实通知没有触发**：Claude 每周剩余约 68%，Codex 约 92%，都高于 30%。通知的 provider 名称、去重、聚合由自动化测试覆盖。

## Readiness for 7-day soak

**结论：可以开始 7 天 soak。** 当前运行的就是最终版本（10-09 00:38:06 启动，托盘中，桥接 + 主动刷新；启动后 2 s 内完成第一次读取）。

soak 期间建议观察：

1. **Claude 主动源的长期限流**：`npm run soak:report` 新增的 Claude 部分会统计每种状态的运行次数（`rate_limited` = 429）。出现 429 时确认退避和暂停按预期工作，并且被动部分和 Codex 不受影响。
2. **Claude 窗口刷新**：5h 窗口会在 01:20 左右第一次重置，每周窗口在 10-13 00:00 重置。确认这时产生 `quota_refreshed_observed`、新的窗口实例，阈值提醒重新生效，并且没有重复提醒。
3. **statusLine 第一次真实数据**：如果用户在终端里使用 Claude Code，snapshot 的 source 会出现 `statusline`，用来确认 `rate_limits` 的真实格式和秒级精度。
4. **Codex detector-v2**：每次 5h 空闲后重新开始使用时，确认只有重锚日志、没有事件；同时记录过原点拟合与平均速度的差距（见 Codex 回归）。
5. **数据库可用性**：本阶段的死锁说明 "监控静默停止" 是需要重点关注的失败方式。soak 报告的 "Longest gaps between polls" 能直接发现它。
6. **启动方式**：soak 期间不要从 Claude Desktop 包内的终端启动 QuotaDock（见 Known limitations 7），在资源管理器中双击，或用 `explorer.exe` 启动。

不阻塞 soak、但需要 GPT 决定的问题：

- 5h 过原点拟合的偏差（Phase 4 是否改用平均速度）。
- 是否在启动时检测 MSIX package identity。
- 对主要使用 Desktop 的用户，被动源无效，是否要把主动刷新的默认值从关闭改为开启。目前遵守 GPT 的决定：默认关闭。

## Completion gate

§80：

| 门禁 | 状态 | 证据 |
|---|---|---|
| A Codex idle→active re-anchor 已修复 | ✅ | 真实 fixture 回归测试 `idle_to_active_reanchors_analysis_segment`；真实运行中 #149 重锚，没有事件 |
| B classifier 升级为 reset-detector-v2 | ✅ | `CLASSIFIER_VERSION = "reset-detector-v2"`；真实库中 v1 和 v2 的派生行都存在 |
| C 历史 reanalyze 完成且没有破坏真实 events | ✅ | v1 9 条事件 / 4 个 cycle 原样保留；v2 的用户可见事件与 v1 相同 |
| D Claude 安全数据源通过 LOCAL TEST | ✅ | `/usage` 零 token 本地命令（外壳每次检查，本阶段所有真实查询全部通过）；statusLine 桥接由真实的 Claude Code 进程调用 |
| E Claude 5h usage 可读取 | ✅ | 真实运行 16→31% |
| F Claude weekly usage 可读取 | ✅ | 真实运行 31→35%；另有 Fable 周额度 |
| G Claude reset timestamps 可读取 | ✅ | `/usage` 分钟精度（真实）；statusLine 秒级（文档，本机未观测到） |
| H 不读取 / 保存 Claude credentials | ✅ | 不访问凭据文件；身份只保存哈希；导出扫描干净 |
| I 不通过监控消耗 Claude quota | ✅ | 不发送 prompt；无效模型名保险；外壳检查 `duration_api_ms = 0`、token = 0 |
| J Codex + Claude 同时出现在 Dashboard | ✅ | 真实窗口截图 |
| K Tray 同时显示两个 provider | ✅ | 真实托盘 tooltip（UI Automation 读取）；菜单两组 |
| L 通知 provider-specific 去重 | ✅ | 自动化测试（重启后不重复、新实例重新提醒、跨 provider 合并）。真实运行中没有达到阈值 |
| M 历史数据 provider-neutral | ✅（有说明） | Claude 存入 `provider_*` 表；Codex 保留 Phase 2 表，读取层统一（History 选择器、事件时间线） |
| N 原有 Phase 3 Codex 功能全部回归通过 | ✅ | Phase 3 的全部测试通过（暂停 / 恢复由自动化测试覆盖）；真实运行 38/38 次轮询，60 / 300 s 节奏，托盘、单实例、正常退出和强制结束都没有孤儿进程。另修复了 Phase 3 就有的启动读取延迟（缺陷 9） |

---

## 附录 A：测试清单

| 套件 | 数量 | 说明 |
|---|---|---|
| Rust `quotadock-core`（`cargo test -p quotadock-core`） | **135** | 单元测试 23（含 Claude 解析、集成、实例跟踪）、`claude_provider` 23、`detector_v2` 6、`engine_scenarios` 25、`product` 21、`storage` 12、`rpc_and_provider` 9、`analytics` 7、`polling` 7、`parity` 2。Phase 3 结束时 81 个 |
| UI（`npm test` in `ui/`） | **24** | 包括新增的 Claude 状态、计划名、额外额度、"所需观察时长" 测试 |
| TS 参考实现（`npm test`） | **83** | 未改动，全部通过 |
| 类型检查 / lint | — | `tsc --noEmit`（UI 和根目录）通过；`cargo clippy` core（all targets）和 app 零警告 |

Claude 相关的主要测试：

- 数据源：
  - `active_usage_is_a_guarded_local_command_and_is_stored`
  - `rate_limits_back_off_and_suspend_the_active_source_after_three`
  - `model_usage_in_the_envelope_disables_the_active_source`
  - `unrecognized_output_is_not_guessed`
  - `api_key_billing_is_never_shown_as_subscription_quota`
  - `logged_out_and_not_configured_states`
  - `a_hung_cli_times_out_and_leaves_nothing_behind`
  - `data_on_the_active_schedule_stays_live_between_background_refreshes`
- 桥接：
  - `passive_bridge_records_are_stored_once_and_ignored_while_paused`
  - `bridge_binary_writes_the_record_and_prints_one_line`
  - `bridge_binary_chains_an_existing_status_line`
  - `stdin_with_a_utf8_bom_is_parsed`
  - `keeps_only_quota_fields`
- 集成（settings.json）：
  - `no_existing_status_line_install_and_remove`
  - `existing_simple_status_line_is_chained_and_restored`
  - `existing_complex_status_line_survives_and_user_edits_are_respected`
  - `missing_file_is_created_and_invalid_json_is_refused`
  - `a_stale_preview_is_refused`
  - `enabling_applies_exactly_the_reviewed_plan`
- 窗口与事件：
  - `minute_precision_from_the_cli_stays_in_the_same_instance`
  - `refresh_after_the_reset_time_is_a_user_visible_event_and_a_new_instance`
  - `a_reset_time_change_alone_is_not_a_refresh`
  - `limit_reached_is_reported_once_and_never_as_availability`
- 产品层：
  - `claude_weekly_threshold_dedup_survives_restart_and_rearms_after_refresh`
  - `codex_and_claude_low_in_one_evaluation_become_one_notification`
  - `tray_takes_the_highest_severity_and_keeps_every_provider`
  - `history_breaks_claude_series_between_window_instances`
  - `events_of_both_providers_share_one_timeline`
  - `a_weekly_claude_forecast_needs_a_day_of_history`
  - `diagnostics_export_includes_claude_without_account_values`
  - `nested_database_access_fails_instead_of_deadlocking`

所有测试都使用临时目录和伪造的 claude 可执行文件，不接触真实的 Claude Code、`~/.claude`、`~/.quotadock` 或用户的 LOCALAPPDATA。

## 附录 D：真实窗口截图

只截 QuotaDock 自己的窗口（DPI-aware PrintWindow），保存在本次会话的 scratchpad 中：

- `real35-overview.png` / `real35-overview-2205.png`：测试中的 Overview，两个 provider 并列。
- `real35-review-dialog.png`：旧版本的启用预览。
- `real35-review-dialog-v2.png`：新版本的启用预览（备份文件名与实际写出的一致）。
- `real35-settings-enabled.png`：设置中的 AI 服务区。
- `real35-overview-final.png`：旧的每周预测误报（"Claude: 每周预测：可能提前耗尽"）。
- `real35-overview-final2.png`：修复后（"至少需要 24 小时的观察"）。

## 附录 B：主要文件

| 路径 | 内容 |
|---|---|
| `crates/quotadock-core/src/claude/` | `env`（干净环境、认证变量检测）、`cli`（定位和运行 claude.exe）、`usage`（`/usage` 外壳检查和解析、auth status）、`statusline`（桥接记录）、`integration`（settings.json 预览、启用、停用）、`tracker`（窗口实例）、`store`（provider 表）、`monitor`（节奏、退避、状态） |
| `crates/quotadock-core/src/bin/quotadock-claude-bridge.rs` | 状态栏桥接 |
| `crates/quotadock-core/src/bin/fake-claude.rs` | 测试替身：ok / apikey / loggedout / ratelimit / inference / unrecognized / slow |
| `crates/quotadock-core/src/analytics/reset_detector.rs` | v2 重锚 |
| `crates/quotadock-core/src/analytics/burn_rate.rs`、`forecast.rs` | 过原点回归 |
| `crates/quotadock-core/src/product/claude_view.rs` | Claude 的 dashboard、托盘、通知输入 |
| `crates/quotadock-core/src/storage/migrations.rs` | v4、v5 |
| `ui/src/components/ClaudeIntegration.tsx` | 启用预览对话框 |
| `scripts/soak-report.mjs` | 新增 Claude 部分；事件只统计当前 classifier |
| `tests/fixtures/real-5h-idle-to-active-2026-10-08.json` | detector-v2 真实 fixture |

## 附录 C：与规格的差异

| 规格 | 实现 | 原因 |
|---|---|---|
| §53 `provider_extra_limits` 表 | `provider_windows` 中 `kind = 'extra'` 的行 | 额外额度与普通窗口的字段完全相同，单独建表没有好处 |
| §53/§54 provider-neutral 存储 | Claude 用新的 `provider_*` 表；Codex 保留 Phase 2 表，读取层统一 | Codex reset 引擎依赖原表列；不迁移就不会有破坏历史的风险 |
| §44 Codex "Enabled: ON" | Codex 没有关闭开关 | V1 中 Codex 是基础 provider；暂停监控已经覆盖 "暂时不监控" 的需求 |
| §48 Comparison view（可选） | 未做 | Overview 已经并列显示两个 provider 的剩余额度；托盘 tooltip 也并列显示 |
| §61 Claude reset detector | 只做窗口实例跟踪 + 3 种事件，不做 reset 分类 | 与 §61 一致："先 record / normalize / persist / display / threshold notification" |
| §66 "禁止 spawn claude" | 可选主动源会启动 `claude -p /usage` | GPT 的决定：它是本地命令、不发送 prompt、带无效模型名作为保险、默认关闭。外壳检查失败会立即停用 |
| §16 statusLine 作为主源 | 本机实际运行中主源没有产生数据，数据来自主动源 | Desktop Code 标签不运行 statusLine；测试期间用户没有用终端里的 Claude Code。见 Known limitations |
| §81 "期间正常使用自己的 Claude Code" | 用户正常使用的是 Claude Desktop（本次开发会话） | 没有为了测试在 CLI 中发送消息（§81 "不要为了测试人工大量消耗"） |
