# Codex Provider Findings

QuotaDock Phase 1B · 2026-10-07 · 本机 Windows 11 实测

本文中的依据标记：

- `[LOCAL TEST]`：本机实际运行并观察到的结果。
- `[SCHEMA]`：本机用 `codex app-server generate-json-schema` 生成的 schema（0.162.0-alpha.2）。
- `[OPENAI CODEX SOURCE]`：openai/codex 源码，tag `rust-v0.162.0-alpha.2`（commit `74e804deeb12`），路径相对于 `codex-rs/`，行号为近似值。
- `[INFERENCE]`：我的判断。
- `UNKNOWN`：无法确定。

## 结论速览

| 项目 | 结果 |
|---|---|
| 4 个 Codex 字段（5h/weekly × remaining/reset） | 全部可自动获取 `[LOCAL TEST]` |
| `ordinaryUsageAllowed` | 可获取，实测为 `true` `[LOCAL TEST]` |
| 最终使用的 transport | `stdio-app-server`：QuotaDock 自己持有的一个常驻 app-server 子进程 |
| managed daemon | 子命令存在，但**未运行**，而且**用 Desktop 自带的 CLI 起不来**，需要先装 standalone CLI `[LOCAL TEST]` |
| daemon-proxy | 已按源码实现（WebSocket over proxy），**没有在真实 daemon 上验证过** |
| 是否触发登录 / OAuth 错误 | 都没有；`auth.json` 在整个 PoC 期间未被改写 `[LOCAL TEST]` |
| 测试 | 83 个测试全部通过（连跑 3 次）；`tsc --noEmit` 零错误 |

---

## Environment

| 项 | 值 |
|---|---|
| Windows | Windows 11 家庭版 中文版 10.0.26200 (build 26200) |
| Codex Desktop | Microsoft Store 包 `OpenAI.Codex` 26.1002.6548.0 |
| Bundled CLI path | `C:\Program Files\WindowsApps\OpenAI.Codex_26.1002.6548.0_x64__2p2nqsd0c76g0\app\resources\codex.exe` |
| CLI version | `codex-cli 0.162.0-alpha.2` |
| VS Code CLI version | `codex-cli 0.154.0-alpha.6.2`（扩展 `openai.chatgpt-26.908.40401-win32-x64`） |
| Standalone CLI | 未安装（PATH 中没有 `codex`，npm 全局也没有 `@openai/codex`） |
| Node / TypeScript | v24.14.0（原生类型擦除，直接运行 `.ts`）/ 5.9.3（只用于 `typecheck`） |
| Rust | 未安装（Phase 1B 不需要） |

## Executable Discovery

`src/codex/discovery.ts` 运行时动态发现，不写死任何带版本号的路径。

| 来源 | 发现方式 | 本机结果 |
|---|---|---|
| desktop（包） | `Get-AppxPackage -Name OpenAI.Codex` → `InstallLocation\app\resources\codex.exe` | 0.162.0-alpha.2 **（选中）** |
| desktop（运行时副本） | `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe` | 0.162.0-alpha.2，与包内文件 sha256 相同（前缀 `97C57E4EB64257BC`） |
| standalone | `where.exe codex`（只接受 `.exe`）；npm 全局 `@openai/codex/vendor/*/codex/codex.exe` | 未找到 |
| vscode | `~/.vscode`、`.vscode-insiders`、`.cursor`、`.windsurf` 下的 `extensions/openai.chatgpt-*/bin/windows-x86_64/codex.exe` | 0.154.0-alpha.6.2 → `unsupported-older` |
| manual | `--codex-path` 或环境变量 `QUOTADOCK_CODEX_PATH`（预留给将来的 Settings） | — |

刻意排除了 `~/.codex/.sandbox-bin/codex.exe`（0.160.0）。它是 Codex Windows sandbox 内部使用的副本，不属于任何"安装"。

**选择规则**

1. 手动指定优先。
2. 否则在 `verified` 和 `unverified-newer` 中取最高版本。
3. 同版本时按来源排序：desktop > standalone > vscode。
4. 默认不会选比已验证版本更旧的版本；`--allow-unverified-version` 可以放开。

**为什么选包路径而不是运行时副本**

- 包路径是"已安装 Codex Desktop"的权威来源，并且可以通过系统 API 动态定位。
- 运行时副本由 Desktop 自己管理，hash 目录随时可能被替换。

两者内容完全相同 `[LOCAL TEST]`。

**Version gating**（`src/codex/version.ts`，按 semver 比较 pre-release）：

- `0.162.0-alpha.2` → `verified`
- 更新的版本 → `unverified-newer`（允许使用，但会给出警告）
- 更旧的版本 → `unsupported-older`（provider 返回 `unsupported-version`）

## Daemon

- **daemon 子命令存在** `[LOCAL TEST]`：`codex app-server daemon {bootstrap,start,restart,update,enable-remote-control,disable-remote-control,stop,version}`；另有 `codex app-server proxy`。
- **daemon 没有运行** `[LOCAL TEST]`：
  - `codex app-server daemon version` 退出码为 1，报错 `failed to connect to %USERPROFILE%\.codex\app-server-control\app-server-control.sock`（os error 10050）。
  - `~/.codex/app-server-control/` 目录不存在。
- **Desktop 在运行不等于 daemon 存在** `[LOCAL TEST]`：
  - Desktop 的 app-server 是它自己的私有子进程：`codex.exe -c features.code_mode_host=true app-server --analytics-default-enabled -c plugins...`，父进程是 Desktop。
  - 没有 `--listen` 参数，即默认 stdio，外部无法连接 `[INFERENCE]`。
  - 源码里也针对 stdio 下 clientInfo 为 `"Codex Desktop"` 的连接有专门分支 `[OPENAI CODEX SOURCE app-server/src/request_processors/initialize_processor.rs]`。
- **用本机现有的 CLI 都启动不了 daemon**。测试在一个隔离的临时 `CODEX_HOME` 里进行，不涉及真实登录态，测试后已删除 `[LOCAL TEST]`：
  - Desktop 自带的 0.162 → `this CLI has no complete local package; install a packaged Codex CLI or use the standalone installer`
  - VS Code 自带的 0.154 → `managed standalone Codex install not found at <CODEX_HOME>\packages\standalone\current\bin\codex.exe … Install it with: irm https://chatgpt.com/codex/install.ps1 | iex`
  - daemon 还要求状态目录的 DACL 受保护，并且只有一条当前用户的 `(OI)(CI)` FullControl ACE `[LOCAL TEST]` `[OPENAI CODEX SOURCE uds/src/windows_socket_validation.rs]`。
  - managed daemon 默认每小时**自动更新**一次 `[OPENAI CODEX SOURCE app-server-daemon/README.md]`。
- **结论**：本机要使用 daemon，必须先用官方安装脚本装 standalone CLI 并启动 daemon。安装属于下载并执行安装脚本，需要你来决定，**本阶段没有安装**。
- **proxy 在没有 daemon 时**：`codex app-server proxy` 约 50 ms 内退出，退出码 1，报错 `failed to connect to socket at …` `[LOCAL TEST]`。

### proxy 的线协议（关键发现）

daemon 控制 socket 的线协议是 **WebSocket**，不是 JSONL `[OPENAI CODEX SOURCE]`：

- `app-server-transport/src/transport/unix_socket.rs` 使用 `tokio_tungstenite::accept_hdr_async_with_config`。
- `app-server-client/src/remote.rs:70` 定义了 `UDS_WEBSOCKET_HANDSHAKE_URL = "ws://localhost/rpc"`。
- `codex app-server proxy` 只是字节中继（`stdio-to-uds/src/lib.rs`）。

所以 `DaemonProxyTransport` 需要在 proxy 的 stdio 上自己完成 HTTP Upgrade 和 RFC 6455 分帧，实现在 `src/codex/transport/codecs.ts`。

| 项 | 状态 |
|---|---|
| proxy 能否 initialize | 已实现；用 RFC 6455 标准测试向量和一个假 WebSocket 服务端验证。**真实 daemon 上 UNTESTED** |
| `account/rateLimits/read` 经 proxy | 同上，UNTESTED |
| 数据是否属于当前账号 | proxy 模式 UNTESTED；stdio 模式为 `match`（见 Authentication） |
| proxy 退出是否影响 daemon | 源码表明不会：只有 stdio 模式在连接关闭时退出 `[OPENAI CODEX SOURCE app-server/src/lib.rs]`；UNTESTED |
| 多次启动是否产生孤儿进程 | proxy 模式 UNTESTED；stdio 模式见下文，没有孤儿进程 |

## Transport

**实际使用的是 `stdio-app-server`** `[LOCAL TEST]`。

| 项 | 观察 |
|---|---|
| 启动命令 | `codex app-server --listen stdio://`（经 `--help` 确认是默认值；`--stdio` 是等价写法） |
| 进程模型 | 一个 QuotaDock 运行时只持有一个常驻子进程，所有轮询复用它；测试验证同一 PID 连续读取 |
| 帧格式 | JSONL：每行一个 JSON，没有 `"jsonrpc":"2.0"` 字段 `[LOCAL TEST]` `[OPENAI CODEX SOURCE app-server-protocol/src/rpc.rs, app-server-transport/src/transport/stdio.rs]` |
| 子进程的子进程 | 空闲读取时只有 `conhost.exe`，没有启动 MCP server 或插件进程 `[LOCAL TEST]` |
| 关闭 stdin | app-server 约 30 ms 内以退出码 0 退出 `[LOCAL TEST]` |
| 强杀 QuotaDock（TerminateProcess） | app-server 立即退出，没有孤儿进程 `[LOCAL TEST]` |
| 两个 QuotaDock 实例同时运行 | 各自持有一个 app-server，退出后都清理干净；前后 `codex.exe` 进程列表与基线一致 `[LOCAL TEST]` |
| 自动选择逻辑 | 先跑 `daemon version`：daemon 在运行就用 proxy，proxy 启动失败再回退到 stdio；否则直接用 stdio |

## Authentication

- **app-server 直接复用 Codex 现有的登录** `[LOCAL TEST]`：`account/read` 返回 `{account: {type: "chatgpt", planType: "plus"}, requiresOpenaiAuth: true}`（email 已脱敏）。
- **没有触发新登录，也没有观察到 OAuth 错误** `[LOCAL TEST]`。PoC 期间约 20 次读取，`auth.json` 的修改时间一直是 2026-10-01 14:12。
- **账号一致性为 `match`** `[LOCAL TEST]`：`account/rateLimits/read` 返回的 `accountId` 与 `account/read` 的 `workspaceRouting.chatgptAccountId` 一致，provider 每次读取都会比对。
- **未登录路径** `[LOCAL TEST]`（在一个没有 `auth.json` 的隔离 `CODEX_HOME` 中测试）：
  - `account/read` 返回 `account: null` → QuotaDock 返回 `{status: "not-authenticated"}`。
  - 直接调用 `account/rateLimits/read` → `-32600 "codex account authentication required to read rate limits"`。这与源码 `[OPENAI CODEX SOURCE app-server/src/request_processors/account_processor.rs]` 一致。
  - API key / Bedrock 登录 → `-32600 "chatgpt authentication required to read rate limits"`，provider 映射为 `unsupported-auth`（源码确认，未实测）。
- **QuotaDock 从不读取 `auth.json`**，也不调用 `account/login/*`、`account/logout`，不传 `refreshToken: true`。

### 多进程 OAuth

`[OPENAI CODEX SOURCE login/src/auth/manager.rs, login/src/auth/storage.rs]`：

- **只有进程内的刷新锁**：`refresh_lock: Semaphore::new(1)`。
- **写 `auth.json` 时没有跨进程锁**：`FileAuthStorage::save` 直接截断重写。
- **缓解机制**：刷新前会先从磁盘重新加载；如果发现文件已被别的进程更新，就跳过本次刷新。遇到 401 时也会先从磁盘重新加载再刷新。
- **残余风险**：如果两个进程几乎同时刷新，服务端返回 `refresh_token_reused`，失败方会被要求重新登录。

所以只要 QuotaDock 用独立 stdio app-server 和 Desktop 共存，**就无法证明存在全局的跨进程刷新互斥**。共享 daemon 只是降低风险的选项。本机目前没有 daemon，所以这个风险被接受，并通过"只开一个 QuotaDock app-server"来压低。

## RPC

| 步骤 | 结果 |
|---|---|
| `initialize` | 可用，约 260 ms `[LOCAL TEST]`。params：`{clientInfo:{name:"quotadock",title:"QuotaDock",version:"0.1.0"}, capabilities:{experimentalApi:false}}`。返回 `userAgent`、`codexHome`、`platformFamily`、`platformOs` `[SCHEMA]` |
| initialize 之前发请求 | `-32600 "Not initialized"` `[LOCAL TEST]` |
| 重复 initialize | `-32600 "Already initialized"` `[LOCAL TEST]` |
| `initialized` 通知 | 会发送，但服务端并不要求：client 通知只被记日志 `[OPENAI CODEX SOURCE app-server/src/message_processor.rs]` |
| 未知方法 | `-32600 "Invalid request: unknown variant …"`（不是 -32601） `[LOCAL TEST]` |
| `account/read` | 可用，约 370 ms，仅在建立连接时调用一次 |
| `account/rateLimits/read` | 可用，约 400–430 ms `[LOCAL TEST]`；属于 stable schema，不需要 `experimentalApi` `[SCHEMA]` |
| 后台读取参数 | `{excludeResetCreditDetails: true, supportsLunaReserve: false}` |

关于 `account/rateLimits/read`：

- **每次调用都是一次实时后端请求**，没有缓存 `[OPENAI CODEX SOURCE app-server/src/request_processors/account_processor.rs: get_account_rate_limits_response; backend-client/src/client/rate_limit_resets.rs: rate_limit_status_url]`。
  - chatgpt.com 下请求的是 `GET …/backend-api/wham/usage`。
  - 不带 `excludeResetCreditDetails` 时还会额外请求一次 `…/wham/rate-limit-reset-credits`。
- **`clientInfo.name` 会进入 app-server 的 User-Agent**（实测为 `quotadock/0.162.0-alpha.2 (Windows 10.0.26200; x86_64) …`）。所以代表 QuotaDock 发出的后端请求会如实标明来源 `[LOCAL TEST]` `[INFERENCE]`。

**安全白名单**：

- app-server 还暴露了高权限方法 `[LOCAL TEST]`，例如 `fs/writeFile`、`process/spawn`、`command/exec`、`config/value/write`、`account/logout`、`account/rateLimitResetCredit/consume`（会用掉 reset credit）、`account/sendAddCreditsNudgeEmail`、`feedback/upload`。
- QuotaDock 在 JSON-RPC 层强制只允许 3 个方法：`initialize`、`account/read`、`account/rateLimits/read`。其他方法在写出之前就被拒绝，有测试覆盖。
- 服务端主动发来的请求（审批、`item/tool/call`、`account/chatgptAuthTokens/refresh`、`attestation/generate`、`currentTime/read`）一律回 `-32601` 拒绝 `[SCHEMA ServerRequest]`。实测中没有收到过此类请求。

**实测收到的通知**：

- `remoteControl/status/changed`：含 `installationId`、`serverName`，日志中已脱敏。
- `account/updated`：`{authMode, planType}`。
- 没有收到 `account/rateLimits/updated`，因为我们不启动任何对话 turn。

## Data

实测返回（已脱敏的完整样本：`tests/fixtures/codex-rate-limits-real-redacted.json`）`[LOCAL TEST]`：

| 字段 | 实测值 |
|---|---|
| `ordinaryUsageAllowed` | `true` |
| `rateLimits.primary` | `{usedPercent: 12, windowDurationMins: 300, resetsAt: 1791393584}` → 2026-10-07T17:19:44Z |
| `rateLimits.secondary` | `{usedPercent: 2, windowDurationMins: 10080, resetsAt: 1791980384}` → 2026-10-14T12:19:44Z |
| `rateLimitsByLimitId` | 只有 `codex`，内容与 legacy `rateLimits` 相同 |
| `rateLimitResetCredits` | `{availableCount: 2, credits: null}`（因为 `excludeResetCreditDetails: true`） |
| 其它 | `planType: "plus"`，`credits: {hasCredits:false, unlimited:false, balance:"0"}`，`spendControlReached: false`，`rateLimitReachedType: null`，`rateLimitUpsell: null` |

几点说明：

- **`usedPercent` 是整数**，schema 中为 int32 `[SCHEMA]`。
- **`resetsAt` 的单位**：schema 没有写明单位。从数值量级和"5h 窗口约 2h50m 后重置"判断是 Unix 秒 `[LOCAL TEST]` `[INFERENCE]`；源码中对应后端字段 `reset_at` `[OPENAI CODEX SOURCE backend-client]`。
  - 解析器保留原值。
  - 超出合理秒级范围（1e9–1e11）的值不转换为 ISO，并给出 warning。
- **`ordinaryUsageAllowed` 可能为 `null`**：只有当后端返回的 `account_id` 和 `user_id` 都与当前登录一致时，它才会被填入 `[OPENAI CODEX SOURCE backend-client/src/client/rate_limit_resets.rs]`。

## Window Mapping

- **实测位置** `[LOCAL TEST]`：`300` 在 `primary`，`10080` 在 `secondary`；多次读取都如此。
- **位置不是协议保证** `[SCHEMA]`：`primary` 和 `secondary` 都只是可空的 `RateLimitWindow`，没有语义约束。它们来自后端的 `primary_window` / `secondary_window` `[OPENAI CODEX SOURCE]`。
- **QuotaDock 只按 `windowDurationMins` 分类**（`normalizeWindows.ts`）：
  - `300` → five-hour，`10080` → weekly，其他 → unknown（保留，不硬归类）。
  - 同一 bucket 中出现两个同类窗口时，该类置为 null 并记录 warning。
  - 分类表可以扩展。

## Bucket Mapping

- **实测只出现 `codex` 一个 `limitId`** `[LOCAL TEST]`。
- **其他 key 由后端动态决定** `[OPENAI CODEX SOURCE backend-client/src/client.rs]`：
  - 主 payload 映射为 `limit_id = "codex"`。
  - `additional_rate_limits[]` 中每一项以它的 `metered_feature` 作为 key，并带上 `limit_name` 和 `normal_model_slug`。
  - legacy `rateLimits` 取 `"codex"` 那个 snapshot，没有时取第一个。
- **QuotaDock 的处理**：
  - `buckets` 保存全部 `rateLimitsByLimitId`。
  - 归一化视图只从 `rateLimitsByLimitId["codex"]` 生成。
  - 缺少 `rateLimitsByLimitId` 时回退到 legacy，并标记 `sourceBucket: "legacy"`。
  - 有 `rateLimitsByLimitId` 但没有 `codex` 时，只有 legacy 的 `limitId` 为 `codex` 才使用它；否则归一化视图留空并给出 warning。
  - 永远不会取"第一个 bucket"。

## Reset Semantics

只记录本次 PoC 实际观察到的事实：

1. 2026-10-07 14:15Z 到 14:42Z 之间约 20 次读取，绝大多数的 `resetsAt` 完全不变：primary `1791393584`，secondary `1791980384` `[LOCAL TEST]`。
2. 有一次例外。14:38:38Z 前后的 watch 测试中，一次读取被标记为 `resetTimestampChanged=true`，同时百分比没有变化。当时没有记录具体数值；随后 5 次间隔 30 秒的读取都恢复为上面的值。**结论：UNCONFIRMED**。watch 模式现在会打印每个变化的 `from -> to`，便于 Phase 2 复现。
3. PoC 期间没有发生任何 reset。所以"reset 之后 weekly `resetsAt` 会不会变化"是 **UNKNOWN**，需要 Phase 2 的历史记录来回答。
4. 两个 `resetsAt` 的分、秒部分相同（`…:19:44`）。只作为事实记录，不推断任何机制。
5. 本账号有 2 个 reset credit（`availableCount: 2`）。兑换方法 `account/rateLimitResetCredit/consume` 的存在，说明存在由用户触发的提前 reset 路径 `[SCHEMA]`。QuotaDock 不会调用它；Phase 2 做 reset 分类时应把 `availableCount` 的变化作为一个输入信号。
6. 按设计约束，QuotaDock 从不根据 `resetsAt` 推断可用性。schema 原文："clients must not infer recovery from percentages or reset times" `[SCHEMA]`。可用性只看 `ordinaryUsageAllowed`；Case E 测试确保 `false` 加过期的 `resetsAt` 仍然是 `blocked`。

## Reliability

| 方面 | 评级 | 理由 |
|---|---|---|
| 数据获取（stdio 路径） | **High** | 实测稳定；方法在 stable schema 中；返回结构与 schema 一致；4 个字段和可用性信号齐全 |
| 接口长期稳定性 | **Medium** | `app-server` 子命令本身标注 `[experimental]`，版本迭代很快（本机约一个月内从 0.154 到 0.162，`.sandbox-bin` 里还留有 0.131 起的二十多个版本），bundled CLI 随 Desktop 自动更新 |
| daemon-proxy 路径 | **Low** | 本机不可用；只基于源码实现，未经真实验证；daemon 生命周期在 README 中标注为 experimental |
| 进程生命周期 | **High** | stdin EOF 自动退出、强杀不留孤儿进程、多实例清理都已实测 |

## Risks

- **app-server experimental API**：`codex app-server` 标注为 `[experimental]`。`account/rateLimits/read` 虽然在 stable schema 中，但字段可能变化。缓解：解析器做防御式处理（未知字段保留在 `raw`，异常值转为 null 加 warning），并有版本门禁。
- **bundled CLI 更新**：Desktop 更新后，WindowsApps 包路径和版本号都会变。缓解：路径动态发现；transport 重连时重新发现；新版本标为 `unverified-newer` 并给出警告。
  - `[INFERENCE]`：常驻子进程会占用旧包目录中的 `codex.exe`，可能推迟旧目录的清理。Phase 2 应在检测到包版本变化后主动重启 transport。
- **daemon 生命周期变化**：daemon 依赖 standalone 安装、私有 ACL 目录、AF_UNIX 108 字节路径限制，并会自动更新。缓解：只在 `daemon version` 成功时才用 proxy，失败时自动回退到 stdio。
- **Microsoft Store 路径变化**：不写死路径（用 `Get-AppxPackage`），并保留手动指定的入口。
- **JSON schema 变化**：每次版本升级都应重新生成 schema 并与 fixture 对比；解析器不要求可选字段存在。
- **OAuth 多进程行为**：独立 app-server 与 Desktop 共存时没有跨进程刷新锁（见上文）；残余风险是极少见的 `refresh_token_reused`，导致某一方需要重新登录。缓解：只运行一个 QuotaDock app-server（Phase 2 加单实例锁）、优先使用 daemon、绝不主动触发刷新。
- **backend 响应变化**：`limitId` 集合、窗口时长、`ordinaryUsageAllowed` 的填充条件都由后端决定。缓解：保留全部 bucket；按时长分类；`null` 表示 unknown。
- **对 Codex 状态的副作用**：QuotaDock 代码本身对 `~/.codex` 是 **0 写入**，但官方 app-server 进程会写自己的状态：
  - 实测：运行后 `logs_2.sqlite`（含 WAL）和 `models_cache.json` 的修改时间更新为运行时刻（首次探测和最后一次运行都观察到） `[LOCAL TEST]`。
  - 在空的 `CODEX_HOME` 中启动，会创建 `installation_id` 和 5 个 sqlite 库 `[LOCAL TEST]`。
  - 源码确认：启动时会打开或迁移 state、logs 等数据库，日志以 TRACE 级别写入 `logs_2.sqlite`；如果触发主动刷新，还可能重写 `auth.json` `[OPENAI CODEX SOURCE app-server/src/lib.rs, state/src/runtime.rs]`。
  - `[INFERENCE]`：可以评估 `-c sqlite_home=<QuotaDock 目录>` 把这些数据库写入隔离出去（源码中有该配置项，**未测试**）。
- **后端负载 / 轮询限制**：每次读取都是真实的后端请求。官方是否有轮询频率限制是 **UNKNOWN**。缓解：默认 5 分钟，失败后按 1/2/5/10/15 分钟退避，30 秒以下必须显式加 `--unsafe-dev-poll`，最低 10 秒；手动刷新 10 秒内会被合并。

## Recommendation

### Recommended QuotaDock Codex architecture

```text
QuotaDock (Phase 2: Tauri)
  └─ CodexProvider
       ├─ discovery ── Get-AppxPackage(OpenAI.Codex) / standalone / vscode / manual
       │               └─ version gate (verified 0.162.0-alpha.2)
       ├─ transport (auto)
       │    ├─ managed daemon running ─► `codex app-server proxy` + WebSocket codec
       │    └─ otherwise ─────────────► ONE persistent `codex app-server --listen stdio://` (JSONL)
       ├─ JSON-RPC client ── allowlist: initialize / account/read / account/rateLimits/read
       │                     server requests refused (-32601)
       ├─ PollingController ── 5 min, backoff 1/2/5/10/15 min, manual refresh (coalesced)
       └─ parseRateLimits → selectCodexBucket("codex") → normalizeWindows(by duration)
            → CodexQuotaSnapshot { availability = ordinaryUsageAllowed, fiveHour, weekly,
                                   buckets (all), resetCredits, raw }
            → detectQuotaTransition (detection only) → Phase 2 history / classification
```

1. **本机默认使用 stdio 回退路径**。它已经实测可靠，并且满足"一个运行时只持有一个常驻子进程"。Phase 2 需要加 QuotaDock 单实例锁，避免出现多个 app-server。
2. **daemon-proxy 暂时保持实现但不依赖它**。如果你以后用官方安装脚本装了 standalone CLI 并启动 daemon，需要先做一次真实验证（initialize、读取、proxy 退出、孤儿进程）再启用。是否安装由你决定。
3. **Phase 2 建议把 transport 和解析逻辑移植到 Rust**，作为 Tauri 后端的一部分（`tokio::process` 加 Windows Job Object `KILL_ON_JOB_CLOSE`）。fixture 和测试用例可以直接复用。TypeScript PoC 保留作为参考实现。Phase 1B 选 TypeScript 是为了零依赖、容易运行和调试。
4. **每次 Desktop 升级后**：重新生成 schema（`codex app-server generate-json-schema`），对比 `GetAccountRateLimitsResponse`，再用 `--save-fixture` 更新真实 fixture。

---

## 附录 A：如何运行

```bash
npm install
npm run -s poc:codex
npm run -s poc:codex -- --watch
npm run -s poc:codex -- --watch --interval 60
npm run -s poc:codex -- --discover
npm run -s poc:codex -- --raw
npm run -s poc:codex -- --save-fixture tests/fixtures/codex-rate-limits-real-redacted.json
npm test
npm run typecheck
```

- `npm install` 只装 `typescript` 和 `@types/node`，用于类型检查；运行本身不需要任何依赖。
- `--watch`：每 5 分钟读取一次；按 Enter 手动刷新，输入 q 再按 Enter 退出。
- `--interval 60`：仅供开发使用，最低 30 秒。
- `--discover`：只列出可执行文件和 daemon 状态，不发 RPC。
- `--raw`：附带脱敏后的原始 RPC 返回。

stdout 只输出 JSON，诊断信息写到 stderr。`accountId` 在终端中做部分掩码（例如 `4782****b0`）。退出码：`0` 成功，`2` 未登录，`1` 其他错误。

## 附录 B：文件结构

```text
src/codex/
  types.ts                     共享类型、只读方法白名单、已验证版本
  discovery.ts                 可执行文件发现 + daemon 状态
  version.ts                   版本解析与门禁
  sanitize.ts                  sanitizeForLog / maskIdentifier
  transition.ts                QuotaTransition 检测（不做分类）
  rpc/jsonRpcClient.ts         请求/响应关联，区分通知、噪声和服务端请求
  transport/transport.ts       CodexTransportManager + 子进程生命周期
  transport/codecs.ts          JSONL 与 WebSocket（RFC 6455）分帧
  transport/stdioAppServer.ts  StdioAppServerTransport
  transport/daemonProxy.ts     DaemonProxyTransport
  parser/parseRateLimits.ts    防御式解析
  parser/normalizeWindows.ts   按 windowDurationMins 分类
  parser/buildSnapshot.ts      bucket 选择 + 快照组装
  polling/pollingController.ts 轮询、退避、手动刷新
  provider/codexProvider.ts    CodexProvider
src/poc/codexUsage.ts          PoC CLI
tests/                         83 个测试（node:test）
tests/fixtures/                合成 Case + 真实脱敏样本
tests/helpers/fakeAppServer.mjs  假 app-server（JSONL / WebSocket 两种模式）
```

## 附录 C：规格要求的测试 Case

| Case | 测试 |
|---|---|
| A：primary=300、secondary=10080 | `normalizeWindows.test.ts` "Case A" |
| B：primary=10080、secondary=null | "Case B"（weekly 识别正确，fiveHour 为 null） |
| C：primary=300、secondary=null | "Case C"（weekly 为 null，不伪造） |
| D：额外 bucket | `parseRateLimits.test.ts` "Case D"（全部保留，归一化只用 codex） |
| E：allowed=false 且 resetsAt 已过期 | "Case E"（仍为 blocked） |
| F：allowed=null | "Case F"（unknown） |
| G：时长 1440 | "Case G"（保留为 unknown 窗口） |

另有覆盖：

- primary/secondary 位置互换、同类重复窗口、只有 legacy、没有 codex bucket、越界值。
- JSON-RPC 的关联、错误、超时、白名单、服务端请求。
- WebSocket 的 RFC 测试向量、分片、ping/pong、close。
- 用假 app-server 测 transport 生命周期（两种分帧）。
- provider 的 transport 选择与回退、轮询退避与节流、变化检测、脱敏、版本比较。

## 附录 D：与规格的差异（均为补充，没有删减）

- **项目放在仓库根目录**，没有再嵌套一层 `quotadock/`。`package.json` 的 name 是 `quotadock`。
- **`CodexQuotaSnapshot` 增加了 3 个字段**：
  - `availability`：由 `ordinaryUsageAllowed` 派生。
  - `sourceBucket`：`"codex"`、`"legacy"` 或 `null`。
  - `warnings`。
- **`CodexInstallation.source` 增加了 `"manual"`。**
- **`QuotaTransition` 增加了 3 个字段**：
  - `resetCandidate`：只表示 usedPercent 下降，不做分类。
  - `availabilityChanged`。
  - `changes`：每项变化的 from → to。
- **新增了几个文件**：`codecs.ts`（daemon socket 需要 WebSocket）、`buildSnapshot.ts`、`transition.ts`、`polling/pollingController.ts`、`sanitize.ts`、`version.ts`。
- **`supportsLunaReserve: false` 显式发送**，没有省略。
- **未登录状态下，每次读取后都会关闭 transport**，以便你在官方客户端登录后能被重新识别。这意味着在该状态下按退避节奏重建子进程；已登录状态下始终复用同一个子进程。
