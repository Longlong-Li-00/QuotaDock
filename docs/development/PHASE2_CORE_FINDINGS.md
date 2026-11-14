# QuotaDock Phase 2 Core Findings

QuotaDock Phase 2 · 2026-10-08 · Windows 11 · Rust 1.99.0（stable-msvc）· Tauri 2

依据标记：

| 标记 | 含义 |
|---|---|
| `[LOCAL TEST]` | 本机实测 |
| `[SCHEMA]` | 本机生成的 app-server schema（0.162.0-alpha.2） |
| `[OPENAI CODEX SOURCE]` | openai/codex 源码，tag `rust-v0.162.0-alpha.2`（commit `74e804de`），路径相对于 `codex-rs/` |
| `[QUOTADOCK HEURISTIC]` | QuotaDock 自己定的规则或阈值，**不是** OpenAI 的行为或指标 |
| `[INFERENCE]` | 推断 |
| `[UNKNOWN]` | 未知 |
| `[EXTERNAL REPORT]` | 外部来源的说法（来自 GPT 的整理），**QuotaDock 没有在本机验证过** |

## 结论速览

| 验收项 | 结果 |
|---|---|
| A 真实读取 Codex quota | ✅ Rust 生产路径实测成功：18 次轮询，0 次失败 `[LOCAL TEST]` |
| 真实 5h 滚动 | ✅ 02:19:44 到期后被正确识别为 NaturalRolloverObserved（MEDIUM），随后识别出滑动锚点，没有误报 `[LOCAL TEST]` |
| B 生产读取不依赖 Node | ✅ 只用 Rust：`codex.exe` 子进程、JSON-RPC 和 SQLite 都在 Rust 后端 |
| C SQLite 保存原始 snapshot | ✅ 存脱敏后的原始 JSON、规范化字段、请求和响应时间戳 |
| D 重启后能恢复历史和 cycle | ✅ 状态机（含 pending candidate）持久化；崩溃遗留的 snapshot 在启动时补分析 |
| E 单次 resetsAt 跳动不误报 | ✅ Test A：A→B→A 被拒绝，只记一条诊断事件 |
| F 合成 reset 场景 | ✅ 规格 Test A–N 和 Case 1–13 全部通过（共 25 个场景测试） |
| G burn rate / forecast / pressure | ✅ 数据不足时如实返回 `insufficient_data` |
| H 前端没有 shell / SQL / RPC 权限 | ✅ 依赖树中没有任何 Tauri 插件；capability 只授予 6 个命令 |
| 测试 | Rust 58 个全部通过，clippy 零警告；TS 参考实现 83 个仍全部通过 |

---

## 0. 开工前的 Review 结论（回答规格第 71 条）

Phase 2 开始时，项目里只有 Phase 1B 的 TypeScript PoC，没有任何 Phase 2 代码。所以以下 review 针对的是 TS 版 `src/codex/transition.ts`：

1. **reset detector 需要替换，而不是修补。** 旧版只比较前后两个 snapshot，没有状态、确认机制和历史，只要 usedPercent 下降就设 `resetCandidate`（违反 1pp 噪声规则）。新引擎 `reset_detector.rs` 整体替换了它。
2. **没有把 resetsAt 当 window ID**，但旧版按 primary/secondary 的位置配对比较窗口。新引擎按 `windowDurationMins` 分类后的窗口类型配对。
3. **没有把时间戳变化当成 reset**，旧版只设一个标志位。
4. **SQLite 原先不存在。** 现在有 `quota_candidates` 表，`engine_state` 里也保存完整状态，pending candidate 都能持久化。
5. **原先没有可供重新分类的原始历史**（Phase 1B 只留下一份 fixture）。现在每个 snapshot 永久保存脱敏后的原始 JSON，`quotadock-dev reanalyze` 可以按当前分类器版本重放全部历史。
6. **IdleOrUninitialized 的实现**：见 §Reset Engine 的滑动锚点检测。
7. **A→B→A 的检测**：见 §Transient resetsAt Test。
8. **需要排除 suspect snapshot 的分析**：burn rate、forecast、cycle 首末值、reset 锚点确认，以及 margin/pressure。全部只使用质量为 `Normal` 的点。

## Architecture

```text
                       Tauri webview (Phase 3 UI; Phase 2: static placeholder)
                                  │  6 narrow commands only (capabilities/default.json)
                                  ▼
 src-tauri (quotadock-app) ── commands ── polling (5 min / 60 s fg, backoff, coalescing)
                                  │
                                  ▼
 crates/quotadock-core
   codex/      discovery → version gate → ONE persistent `codex app-server --listen stdio://`
               (Job Object, no console, no shell) → read-only JSON-RPC (3-method allowlist)
               → parser → normalizer (byte-for-byte parity with the TS reference)
   pipeline    Provider → Validate/Normalize → Persist raw (tx 1) → Engine.step → Persist derived (tx 2)
   analytics/  transitions → reset_detector (observation state machine, reset-detector-v1)
               → observed cycles → burn_rate (OLS) → forecast (+ pressure heuristic)
   storage/    %LOCALAPPDATA%\QuotaDock\quotadock.sqlite3 (WAL, FK, busy_timeout, migrations)
```

- React/前端**不**管理 codex.exe，**不**访问 SQLite，**不**持有 RPC transport。这些全部在 Rust 后端。
- 所有分析都只读本地 SQLite，**不会为了计算而额外请求后端**。
- 每个数据库只允许一个引擎写入：`quotadock.lock` 以独占共享模式打开，保证同一个 snapshot 不会被两个进程分析 `[LOCAL TEST: only_one_engine_per_database]`。

## Rust Port

| TS 参考模块（保留） | Rust 生产模块 | 状态 |
|---|---|---|
| `parser/parseRateLimits.ts` | `codex/parser.rs` | 已移植，**与 TS 完全对照**（见下） |
| `parser/normalizeWindows.ts`、`parser/buildSnapshot.ts` | `codex/normalize.rs` | 已移植并对照 |
| `sanitize.ts` | `codex/sanitize.rs`（另加 account fingerprint） | 已移植 |
| `version.ts` | `codex/version.rs` | 已移植 |
| `discovery.ts` | `codex/discovery.rs` | 已移植；daemon 探测**不移植**，V1 不使用 daemon |
| `rpc/jsonRpcClient.ts` | `codex/rpc.rs` | 已移植（JSONL，泛化到任意 AsyncRead/AsyncWrite，可以在内存中测试） |
| `transport/stdioAppServer.ts` + `transport.ts` | `codex/process.rs` | 已移植，并新增 Windows Job Object 约束 |
| `provider/codexProvider.ts` | `codex/provider.rs` | 已移植（只保留 stdio） |
| `polling/pollingController.ts` | `polling/controller.rs` | 已移植（tokio，新增前台 60 秒模式） |
| `transition.ts` | `analytics/transitions.rs` + `reset_detector.rs` | **重写**（旧版语义不再满足规格） |
| `transport/daemonProxy.ts` + `codecs.ts`（WebSocket） | — | **只保留在 TS 中**：experimental、默认关闭、未在真实 daemon 上验证；V1 不尝试 daemon，也不会安装 standalone CLI |
| `src/poc/codexUsage.ts` | `bin/quotadock-dev.rs` | 开发入口已移植 |

**对照测试**（`tests/parity.rs`）：
- `node scripts/gen-parity.ts` 用 TS 实现生成 13 份 fixture 的输出：合成的 Case A–G 加上若干边界情况，以及真实的脱敏样本。结果存为 `tests/fixtures/parity/ts-normalized.json`。
- Rust 对每份 fixture 逐字段比较：fiveHour、weekly、全部 bucket（含 raw）、availability、sourceBucket、resetCredits，**warning 文本也必须逐字相同**。
- 结果：**13/13 完全一致，没有语义差异需要解释** `[LOCAL TEST]`。

## Database

| 项 | 值 |
|---|---|
| 路径 | `%LOCALAPPDATA%\QuotaDock\quotadock.sqlite3`，通过 Known Folder API（`dirs::data_local_dir`）解析，不硬编码用户名；可用 `QUOTADOCK_DATA_DIR` 或 `--db` 覆盖 |
| 引擎 | rusqlite 0.40（bundled SQLite） |
| 迁移 | `schema_migrations` 表；当前 **v1 `phase2_core`**；每个迁移在独立事务中执行，可以重复调用 |
| PRAGMA | `journal_mode=wal`，`foreign_keys=ON`，`busy_timeout=5000`，`synchronous=NORMAL`（均为 `[LOCAL TEST]` 读回的结果） |
| 时间 | 全部是 UTC Unix 毫秒；服务端的秒级 resetsAt 乘以 1000 存储；本地时间只用于显示 |

**表结构（v1）**

| 表 | 用途 |
|---|---|
| `snapshots` | 原始观测：请求开始、响应收到、captured 三个时间戳；account fingerprint；`account_check`；可用性；5h 和 weekly 的 used/remaining/reset/duration；reset credit 数；bucket ids；`bucket_json`；`raw_sanitized_json`；warnings。派生列：总质量和每个窗口的质量、`analysis_status`、`analysis_json`、`analysis_classifier_version` |
| `poll_runs` | 每次轮询一行：`success`、`network_error`、`authentication_error`、`rpc_error`、`parse_error`、`provider_unavailable`。失败时**不写虚假 snapshot** |
| `observed_cycles` | ObservedCycle：`observed_started_at` 只表示"QuotaDock 第一次看到这个周期的时间"；`nominal_window_start_ms` 是推导值（reset 减去时长，只基于已确认且非滑动的锚点），**不是权威值**；还有 start/end reason、事件 key、置信度、状态（open/closed/voided） |
| `quota_events` | 事件（含证据 JSON、可见性、置信度、`classifier_version`、`superseded_by_event_key`） |
| `quota_candidates` | 候选的 pending、confirmed、rejected 状态及证据 |
| `window_tracks` | 每个窗口当前的观察状态，供查询使用 |
| `engine_state` | 状态机完整状态的 JSON（重启时用来恢复） |
| `settings` | 保留策略结构：`{"raw_snapshot_days":90,"cleanup_enabled":false}`，默认不清理 |

**外键与保留策略**：事件和 cycle 对 snapshot 的引用都是 `ON DELETE SET NULL`。将来按保留策略删除原始 snapshot 后，事件和证据仍然保留 `[LOCAL TEST: foreign_keys_set_null_on_snapshot_retention_delete]`。

## Real Codex Test

**成功。** 详细数据见下面的 History 一节。

- **生产读取路径**：`quotadock-dev`（纯 Rust）→ 动态发现 Store 包里的 `codex.exe` 0.162.0-alpha.2 → 启动一个常驻 stdio app-server → `initialize` → `account/read` → `account/rateLimits/read`，参数为 `{excludeResetCreditDetails:true, supportsLunaReserve:false}` `[LOCAL TEST]`。
- **冒烟测试**（临时数据库，2026-10-07T16:14Z）：
  - 结果：`availability=allowed`，5h 12%，weekly 2%，2 个 reset credit，app-server 版本 0.162.0-alpha.2，plan plus。
  - 读取结束后系统里只剩 Desktop 自己的两个 `codex.exe`，没有残留进程 `[LOCAL TEST]`。

## History

真实数据按生产节奏采集，写入默认数据库 `%LOCALAPPDATA%\QuotaDock\quotadock.sqlite3`：
- 时间段：2026-10-07T16:23:24Z – 17:48:35Z，每 300 秒一次，共 **18 次轮询**。
- 结果：**18 个 snapshot，0 次失败**；轮询结束后系统里只剩 Desktop 自己的 `codex.exe` `[LOCAL TEST]`。
- 下表时间为本地时间（UTC+9）。

| # | 本地时间 | 5h used | 5h resetsAt（服务端秒） | weekly | 引擎判断 |
|---|---|---|---|---|---|
| 1 | 01:23:24 | 12% | 1791393584（02:19:44） | 2% @ 10-14 21:19:44 | 首次观测，锚点未确认 |
| 2–12 | 01:28 – 02:18 | 12% | 不变 | 不变 | 第 2 次读取起锚点确认；Active；无事件 |
| 13 | 02:23:33 | **0%** | **1791411814** = 本次读取时间 + 18000.85 秒 | 不变 | 已越过之前的 reset 时间，且用量下降 12pp → **Rollover 候选**（未确认） |
| 14 | 02:28:33 | 0% | 1791412114（+300 秒） | 不变 | **NaturalRolloverObserved 确认**；置信度 **MEDIUM**，因为新锚点不稳定，所以不给 HIGH |
| 15 | 02:33:34 | 0% | +301 秒 | 不变 | **SlidingAnchorDetected**（3 个样本，偏移方差 0.05 s²）；IdleOrUninitialized |
| 16–18 | 02:38 – 02:48 | 0% | 每次 +300/301 秒 | 不变 | 持续滑动，不产生新事件；倒计时和 forecast 都不使用这个值 |

- **事件**：
  - 1 个用户可见事件：`natural_rollover_observed`（five_hour，MEDIUM）。
    - observedAt 是 02:23:33，也就是**首次看到新状态的时间**，不是"精确的 reset 时间"。
    - 证据完整保存：前后的 used 和 resetAt、`crossedPreviousResetTime:true`、`creditsDelta:0`、两个确认点。
  - 1 个诊断事件：`sliding_anchor_detected`。
  - 没有 TransientBackendState、WindowScheduleChanged、UnscheduledQuotaRefresh 或 ResetCreditRedeemed。
- **ObservedCycle**：
  - 5h 旧周期在 #12 结束（end_reason `natural_rollover_observed`）。新周期从 #13 开始，start_reason 为 `natural_rollover_observed`，置信度 medium。
  - weekly 周期从 #1 开始，start_reason 为 `first_observation`，置信度 low。真实开始时间未知，QuotaDock 只知道自己第一次看到它的时间。
- **最终的 forecast**：
  - 5h：`insufficient_data`。新周期只有 6 个点、25 分钟，没有达到 30 分钟的最低跨度。
  - weekly：burn 为 0.00 pp/h，状态 `not_projected`，没有生成无限远的日期；pressure 0，风险 Low。weekly 锚点是已确认的 10-14 21:19:44。
- **本阶段对后端的请求总数**：
  - 默认库：18 次。
  - 冒烟测试库：1 次。
  - Tauri 外壳试运行库：1 次。
  - 所有分析和查询都只读本地 SQLite。
- **Tauri 外壳试运行**（临时数据目录）：
  - 启动后有两个子进程：WebView2 和一个 app-server。
  - 启动时的那次读取已写入 SQLite。
  - 用 WM_CLOSE 关闭后正常退出（退出码 0），app-server 子进程同步结束 `[LOCAL TEST]`。

## Reset Engine

`reset-detector-v1` 是一个**观察状态机**，**不是**对 OpenAI 服务端状态机的建模。核心性质：
- 引擎是纯函数 `step(state, observation)`，不读系统时钟，所有时间都取自 snapshot。
- 因此实时处理和从原始 snapshot 重放得到的结果完全一致 `[LOCAL TEST: replay_is_deterministic, reanalysis_reproduces_the_same_derived_history]`。

处理流程：

```text
Observation (从已存的 snapshot 行重建)
 → 账号上下文：fingerprint 变化时发 AccountChanged，关闭旧上下文的 cycle，不做跨账号比较
 → reset credit 跟踪：减少后开启关联窗口；增加或未关联的减少只发诊断事件
 → bucket 拓扑：新的 bucket 集合也要求连续 2 次相同才确认
 → 对每个窗口（five_hour、weekly，按时长分类）：
     1. 刚确认过的变化是否已经回到原状态 → BackendSnapshotReconciliation（撤销 cycle 边界）
     2. 处理 pending candidate：回到原值 → Rejected（TransientBackendState）；
        继续保持新值 → 累计确认次数；变成第三个值 → 拒绝后重新评估
     3. 已接受点之间的 A→B→A（例如一次性的用量尖峰）→ 把 B 标为 SuspectTransient
     4. 滑动锚点（used=0 且 resetsAt ≈ now + duration，并随时钟同步移动）→ 只接受观测，不开候选
     5. 新变化 → 打开候选：
        Rollover        已越过之前的 reset 时间，且 reset 前移或用量大幅下降
        QuotaRefresh    用量下降 ≥ 10pp，但没有越过之前的 reset 时间
        ScheduleChange  只有 resetsAt 改变（用量变化在噪声范围内或上升）
        Unknown         用量下降 2–9pp，或 reset 从有变无
```

**事件类型和可见性**

| 事件 | 默认可见性 | 是否建立 cycle 边界 |
|---|---|---|
| `NaturalRolloverObserved` | 用户 | 是 |
| `ResetCreditRedeemed` | 用户 | 是 |
| `UnscheduledQuotaRefresh` | 用户 | 是 |
| `WindowScheduleChanged` | 诊断 | 否 |
| `SlidingAnchorDetected` | 诊断 | 否 |
| `TransientBackendState` | 诊断 | 否 |
| `BackendSnapshotReconciliation` | 诊断 | 撤销边界 |
| `BucketTopologyChanged` | 诊断 | 否（缺失的窗口除外） |
| `AccountChanged` | 诊断 | 关闭旧上下文 |
| `ResetCreditCountChanged` | 诊断 | 否 |
| `UnknownQuotaTransition` | 诊断 | 否（但 burn rate 的分析段会重新开始） |

**确认规则（全部为 `[QUOTADOCK HEURISTIC]`，集中定义在 `analytics/config.rs`）**

| 规则 | 默认值 |
|---|---|
| 新 resetsAt 视为确认锚点 | 连续 2 次成功读取得到**完全相同的值**。比较用精确相等，不用"变化超过 N 秒"这类阈值（遵守规格第 21 条） |
| 噪声容差 | 下降 ≤ 1pp（`percent_noise_tolerance_pp`）。依据：usedPercent 是后端浮点值 `.round()` 后的结果 `[OPENAI CODEX SOURCE app-server-protocol/src/protocol/v2/account.rs:763]` |
| 大幅下降 | ≥ 10pp（`substantial_drop_pp`），只算强证据，单凭它不足以判定 reset |
| Rollover / ScheduleChange / Unknown 的确认 | 开候选后再读到 1 次一致的值（共 2 次） |
| UnscheduledQuotaRefresh 的确认 | 共 3 次读取（开候选 + 2 次）。先等 credit 关联窗口结束，避免迟到的 credit 更新把一次已经宣布的刷新变成第二个事件 |
| ResetCreditRedeemed | 用量刷新加上 ±2 次读取内的 credit 减少，**立即**以 HIGH 确认（规格的强证据例外）。两个窗口共用**同一个**账号级事件，事件按 key upsert |
| 滑动锚点 | ≥ 3 个样本，容差 ±120 秒（`sliding_anchor_tolerance_secs`） |
| 窗口缺失 | 连续 3 次缺失才算长期拓扑变化；短暂缺失只标记 TemporarilyMissing |
| backend reconciliation | 确认后 3 次读取以内回到确认前的状态 |

**置信度 `[QUOTADOCK HEURISTIC]`**

| 事件 | 置信度 |
|---|---|
| Rollover | HIGH：大幅下降，且新锚点稳定、非滑动；MEDIUM：只满足其中一项；LOW：两项都不满足 |
| UnscheduledQuotaRefresh | MEDIUM：新锚点稳定或 reset 未变；LOW：锚点仍在变化 |
| WindowScheduleChanged | MEDIUM |
| ResetCreditRedeemed | HIGH |
| TransientBackendState、Unknown、单独的 credit 变化 | LOW |

**两条始终不变的规则**：
- `ordinaryUsageAllowed` 永远只来自后端，引擎没有任何写它的代码路径（Test F，以及 `expired_but_blocked_through_the_pipeline`）。
- 分类结果永远不会写成 "global reset" 或 "promotional reset"。UnscheduledQuotaRefresh 的证据里固定写 `"cause":"unknown"`。

## Transient resetsAt Test

**Phase 1B 那种 glitch 能被正确拦截，不会误报** `[LOCAL TEST: a_transient_timestamp_is_rejected]`。

测试序列：`12/T1 → 12/T1+600s → 12/T1`。
- 第 2 次读取：只开一个 `ScheduleChangeCandidate`，锚点保持 T1。这一点的质量标为 PendingConfirmation，被排除在分析之外。
- 第 3 次读取：值回到 T1，候选被 **Rejected**。只记一条诊断事件 `TransientBackendState`（LOW），第 2 个 snapshot 的 5h 质量改为 `suspect_transient`。
- 不产生 reset，不产生 `WindowScheduleChanged`，不产生 cycle 边界，没有任何用户可见事件。
- 两次 resetsAt 变化都会按规格第 61 条打印诊断日志（window、old、new、deltaSeconds、usedBefore、usedAfter、creditBefore、creditAfter、classification）。

相关测试：
- 完整状态的 A→B→A（Test K：34/T1 → 25/T2 → 34/T1）同样不产生 cycle 边界。
- 用量的单次尖峰（12 → 20 → 12）也会被识别为 transient。

## Burn Rate

- **估计方法**：普通最小二乘（OLS）求 usedPercent 对时间的斜率，单位是 pp/h。
- **为什么选 OLS**：
  - 异常点已经由状态机在上游排除：只使用当前 cycle 分析段内 `Normal` 质量的点，transient、pending、缺失窗口和账号不一致的点都不参与，也绝不跨越已确认的 rollover、refresh 或 credit 边界。
  - usedPercent 被四舍五入成整数，误差大致对称，OLS 可以把它平均掉。相比之下，Theil–Sen 这类"成对斜率中位数"的方法在阶梯状数据上会偏向 0，因为相邻的大多数点对数值相同。
  - 能自然处理不均匀的轮询间隔。
  - 不需要任何统计依赖，代码约 20 行，容易测试。
- **指标**：`recent_6h_rate`、`recent_24h_rate`、`current_cycle_rate`；weekly 另外给出 pp/day。
- **周期内的负斜率**（例如 12 → 11）按修正处理，报告为 0。
- **最低数据要求 `[QUOTADOCK HEURISTIC]`**：至少 3 个 Normal snapshot，观察跨度至少 30 分钟。不满足时返回 `insufficient_data`。第一个 snapshot 显示 12%，**不会**被拿来反推速率。
- **用量稳定时**：burn = 0，状态为 `not_projected`，**不会**生成无限远的日期。

## Forecast

- **耗尽时间**：`remaining = 100 − used`；需要观察到至少 2pp 的消耗才会给出。耗尽时间 = 最新观测时间 + remaining / rate。
  - 5h 窗口用整个当前 cycle 的速率。
  - weekly 有 24 小时数据时用最近 24 小时的速率，否则用 cycle 速率。这是 `[QUOTADOCK HEURISTIC]`，用来反映"当前节奏"。
- **服务端 reset 时间**：只使用已确认锚点（连续 ≥2 次相同、非滑动、仍在未来）。
  - 滑动、未确认或已过期的锚点一律返回 `reset_schedule_unconfirmed`，不计算 margin 和 pressure。
  - 耗尽时间本身仍然照常给出。
- **margin_hours** = (耗尽时间 − reset) / 小时：正数表示预计能撑过重置时间，负数表示可能在重置前耗尽。（Phase 3 修正了符号，最初实现为 reset − 耗尽时间。）
- **quotaPressure** `[QUOTADOCK HEURISTIC，不是 OpenAI 指标]`：pressure = rate × 距 reset 的小时数 / remaining。
  - 小于 0.7 为 Low，0.7–1.0 为 Watch，大于 1.0 为 AtRisk。
  - 输出里带有固定文字 `heuristic: "QuotaDock heuristic, not an OpenAI metric…"`，UI 必须展示。
- **置信度 `[QUOTADOCK HEURISTIC]`**：
  - High：至少 12 个点、跨度至少 2 小时，并且锚点连续确认 ≥3 次。
  - Medium：至少 6 个点、跨度至少 1 小时。
  - 其余为 Low。

## Restart Recovery

| 场景 | 结果 |
|---|---|
| Test N：pending candidate → 关闭程序 → 重启 → 下一次读取确认 | ✅ `WindowScheduleChanged` 正常确认，candidate 在 SQLite 中从 `pending` 变为 `confirmed` `[LOCAL TEST: n_restart_with_a_pending_candidate_completes_confirmation]` |
| 崩溃发生在原始 snapshot 提交之后、分析之前 | ✅ 启动时自动补分析残留的 `pending` snapshot `[LOCAL TEST: crash_between_raw_commit_and_analysis_is_recovered_on_startup]` |
| 分析失败 | ✅ 原始 snapshot 保留并标记为 `analysis_status='failed'`；内存状态不变；可以用 `reanalyze` 重做 |
| 状态 JSON 往返 | ✅ 每一步之后状态都做 JSON 序列化再反序列化，并断言完全相等 |
| 重新分析 | ✅ 清空当前分类器版本的所有派生数据后从原始 snapshot 重放，事件、证据和状态完全一致 |

## Security

- **RPC 白名单**：
  - 只允许 `initialize`、`account/read`、`account/rateLimits/read` 三个请求和 `initialized` 一个通知。
  - 其他方法在写入 stdin 之前就被拒绝，测试覆盖了 logout、login、reset credit consume、fs/writeFile、process/spawn、command/exec、config/value/write、thread/start。
  - 服务端主动发来的请求一律回 `-32601` 拒绝 `[LOCAL TEST]`。
  - 从不发送 `refreshToken:true`；`experimentalApi:false`。
- **进程约束**：
  - 不经过 shell；带 `CREATE_NO_WINDOW`；只有 stdin、stdout、stderr 三条管道。
  - 放进 **Windows Job Object（`KILL_ON_JOB_CLOSE`）**，并设置 `kill_on_drop`。
  - 实测：辅助进程用 TerminateProcess 强杀后，一个**故意忽略 stdin EOF** 的子进程也被立即结束 `[LOCAL TEST: job_object_kills_the_child_when_quotadock_dies]`。
  - 正常 stop 会关闭 stdin，等待 3 秒后再 kill。
- **前端权限**：
  - `quotadock-app` 的依赖树中**没有任何 `tauri-plugin-*`**。
  - `build.rs` 只为 6 个命令生成权限；`capabilities/default.json` 只授予这 6 个；`withGlobalTauri:false`；CSP 只允许 self 和 ipc `[LOCAL TEST: gen/schemas/capabilities.json]`。
  - 前端没有任何途径执行 shell、读写文件、写 SQL 或发起任意 RPC。
- **数据库隐私**：
  - 完整的 accountId **只存在于内存中**。
  - 数据库只存 `account_fingerprint`：对 `"quotadock:account:v1:" + accountId` 做 SHA-256，取前 128 bit（32 个十六进制字符）。
  - 原始 JSON 写盘前经过 `sanitize_value`，accountId、email、installationId、token 等都替换成 `[REDACTED]`。
  - 测试会扫描整个数据库目录（含 WAL 文件）的字节，确认里面不包含完整 accountId `[LOCAL TEST: snapshot_persistence_and_privacy]`。
  - 日志、事件证据和 UI 只出现 8 个字符的短 fingerprint。
- **对 `~/.codex` 的写入**：
  - QuotaDock 自身为 **0 写入**。
  - 官方 app-server 进程会写它自己的 `logs_2.sqlite` 和 `models_cache.json`（Phase 1B 已实测），这属于 Codex 自身的行为，只记录不干预。

## /wham/usage Lifecycle Assumptions

### Confirmed

- `resetsAt` 是后端提供的值：客户端把 `primary_window` / `secondary_window` 的 `reset_at` 原样透传，不用 `reset_after_seconds` 重新计算 `[OPENAI CODEX SOURCE backend-client/src/client.rs map_rate_limit_window]`。
- `usedPercent` 是后端浮点值 `.round()` 后的整数 `[OPENAI CODEX SOURCE app-server-protocol/src/protocol/v2/account.rs:763]`。
- `ordinaryUsageAllowed` 是权威的可用性信号 `[SCHEMA]`："clients must not infer recovery from percentages or reset times"。
- 本账号观测到的窗口时长只有 300 和 10080 分钟，bucket 只有 `codex` `[LOCAL TEST]`。
- 每次 `account/rateLimits/read` 都是一次实时后端请求 `[OPENAI CODEX SOURCE account_processor.rs]`。

### Observed

- 本机曾出现过一次 resetsAt 的瞬时变化：百分比不变，之后恢复原值，随后连续 5 次读取稳定 `[LOCAL TEST, Phase 1B]`。
- **5h 窗口自然到期后的空闲表现** `[LOCAL TEST]`，2026-10-07，Plus 账号，期间没有使用 Codex：
  - 返回 `usedPercent = 0`，`resetsAt = 读取时间 + 300 分钟`（向上取整到秒，偏差 +0.3 到 0.9 秒）。
  - 每次请求 resetsAt 都随读取时间前移约 300 秒，连续 6 次读取都是这样，偏移方差 0.05 s²。
  - 也就是说，**到期后的空闲 5h 窗口使用滑动锚点**，在开始使用之前并不存在固定的 reset 时间。
  - 这只是**一个账号的一次观测**，不能当作协议保证。
- **到期变化的发现延迟** `[LOCAL TEST]`：新状态在旧 resetsAt 之后 3 分 49 秒才被看到，这是轮询间隔造成的。服务端实际切换的时刻未知。
- **两个窗口的起点重合** `[LOCAL TEST]`：weekly 的 resetsAt 减 7 天，与旧 5h 窗口的 resetsAt 减 5 小时，**恰好相等**，都是 2026-10-07T12:19:44Z。也就是说两个窗口是在同一时刻开始的。
  - `[INFERENCE]` 这与"首次使用同时开启 5h 和 weekly 窗口"的说法一致，但没有被证明。
- **18 次连续读取中没有出现瞬时变化** `[LOCAL TEST]`。Phase 1B 那种 glitch 很少见，目前约 40 次读取里只出现过 1 次。
- A→B→A 形式的后端额度状态 `[EXTERNAL REPORT]`：有公开报告提到过，QuotaDock 尚未在本机观察到"百分比也跳回"的情况。
- 滑动的 resetsAt（空闲窗口）`[EXTERNAL REPORT]`：有公开报告提到过，见下面的 Unknown 和真实观测部分。

### Heuristic

以下全部是 `[QUOTADOCK HEURISTIC]`：
- 新 resetsAt 需要连续 2 次相同才确认。
- 1pp 噪声容差；10pp 视为大幅下降。
- 滑动锚点检测（3 个样本，±120 秒）。
- 事件置信度的分级。
- credit 关联窗口（±2 次读取）；reconciliation 窗口（3 次读取）；窗口缺失需连续 3 次确认。
- burn rate 的最低数据要求（3 个点、30 分钟、2pp）。
- pressure 分档阈值（0.7 / 1.0）；forecast 置信度的分级。

### Unknown

以下全部是 `[UNKNOWN]`，不做猜测：
- `/wham/usage` 对空闲或未初始化窗口的表示**是否在所有情况下都一样**。目前只有 5h 窗口在一个账号上自然到期后的一次观测，结果是滑动锚点（见 Observed）。以下几种情况都还未知：
  - weekly 窗口空闲时的表示；
  - reset credit 兑换后的空闲表示；
  - 首次使用后 resetsAt 是否固定为"使用时刻 + 时长"，这一点尚未观察到。
- weekly 窗口正常到期后如何初始化下一个周期。
- 全局 reset 时 resetsAt 是否会变化。
- 服务端发生 reconciliation 的原因。
- 后端的时钟和计量架构；服务端时钟与本机时钟的偏差（我们只用本地的响应接收时间）。
- reset credit 兑换后 API 的确切表现，包括 credit 数和额度是否原子更新，以及 resetsAt 如何变化。
- 未来新 bucket 的含义。
- daemon-proxy 在真实 daemon 上的表现。

## Known Unknowns

同上面的 Unknown 列表，另外几项：
- **轮询频率上限**：官方是否限制 `wham/usage` 的请求频率，未知。
- **多进程 OAuth 刷新**：本机没有 daemon，QuotaDock 的 app-server 与 Desktop 并存。源码显示写 `auth.json` 时没有跨进程锁，只有"刷新前先从磁盘重新加载"这一缓解 `[OPENAI CODEX SOURCE login/src/auth/*]`。所以极低概率的 `refresh_token_reused` 风险仍然存在。现在已经用"每个数据库只允许一个引擎"来避免多个 QuotaDock app-server 同时运行。
- **Store 更新时常驻子进程会占用旧包目录**：`[INFERENCE]`，未实测。

## Phase 3 Recommendation

**结论：可以进入 Phase 3（Tauri UI + 托盘 + 通知）。** 数据层已经满足规格第 69 条 A–K 的全部验收项，并在一次真实的 5h 滚动中表现正确：没有误报，置信度没有被夸大，滑动锚点没有被当成倒计时。

进入 Phase 3 时建议遵守以下几点：

1. **倒计时只用 `confirmedResetAtMs`。**
   - `resetSchedule` 为 `sliding` 时显示"开始使用后才开始计时"这类文案，不要显示倒计时。
   - 为 `unconfirmed` 或 `passed` 时不显示倒计时。
2. **可用状态只看 `availability`**，也就是 `ordinaryUsageAllowed`。
3. **通知只针对 3 种用户可见事件**，并且要带上置信度。
   - `NaturalRolloverObserved` 每个 5h 周期都会出现，建议加一个产品层面的阈值，例如只在之前用量 ≥ X% 时才通知。具体数值由你决定。
   - `pressure` 和 `risk` 在 UI 上必须标注"QuotaDock 估算"。
4. **应用层加单实例**。数据库锁已经能防止两个引擎同时写入，但第二个应用实例目前会直接启动失败，需要在应用层处理。
5. **前台时把轮询间隔切到 60 秒**（已经实现，见 `set_foreground`），后台保持 5 分钟。
6. **继续保持**：daemon-proxy 关闭；不安装 standalone CLI；不调用任何高权限 RPC。
7. **Phase 3 期间让 QuotaDock 连续运行至少一周（soak）**：
   - 目的是观察 weekly 的真实滚动和空闲表现（目前仍是 UNKNOWN），以及 transient 出现的频率。
   - 需要调整规则时，升级 `CLASSIFIER_VERSION`，然后用 `reanalyze` 重放历史。
8. **发布构建、安装包和自动更新**仍属于后续阶段。注意 Store 版 Desktop 更新后，`codex.exe` 的路径会变：provider 在子进程重启时会重新做 discovery，这条路径还需要在一次真实更新中验证。

---

## 附录 A：如何运行

```bash
npm run poc:phase2
npm run poc:phase2 -- watch --count 6 --interval 300
npm run poc:phase2 -- summary
npm run poc:phase2 -- events --all
npm run poc:phase2 -- history --limit 50
npm run poc:phase2 -- reanalyze
npm run test:rust
npm test
cargo build -p quotadock-app
```

- `npm run poc:phase2`：做一次真实读取，写入 SQLite，然后打印摘要。
- `watch`：开发用的连续读取；间隔至少 30 秒，加 `--unsafe-dev-poll` 可以降到 10 秒。
- `summary`：只读本地 SQLite，不请求后端。
- `events --all`：列出事件，包含诊断事件。
- `reanalyze`：用当前分类器版本重放全部原始 snapshot。
- `npm run test:rust`：Rust 测试，共 58 个。
- `npm test`：TS 参考实现的测试，共 83 个。
- `cargo build -p quotadock-app`：构建 Tauri 外壳（Phase 2 只有占位页面）。

## 附录 B：测试清单（Rust，58 个）

| 文件 | 数量 | 覆盖内容 |
|---|---|---|
| `engine_scenarios.rs` | 25 | 规格 Test A–N；Case 1、11；噪声和中等下降；用量尖峰；已确认变化的回退（schedule 和 rollover）；滚动后进入空闲滑动；未关联的 credit；未验证身份的 snapshot；重放确定性 |
| `storage.rs` | 11 | 迁移和 PRAGMA；单引擎锁；持久化和隐私扫描；失败轮询不写 snapshot；provider 失败记为 poll_run；过期但被阻止，以及 null 可用性；Test N 重启；崩溃恢复；重新分析；burn rate 排除 transient 点且不跨边界；外键 SET NULL |
| `rpc_and_provider.rs` | 8 | 请求和响应关联；白名单；拒绝服务端请求；错误、超时和 EOF；常驻子进程复用并停止；未登录；子进程崩溃；Job Object 强杀 |
| `analytics.rs` | 7 | OLS；数据不足；用量稳定；margin、pressure 和风险分档；滑动或未确认锚点不参与计算；负斜率；weekly 的 pp/day |
| `polling.rs` | 5 | 立即首读后每 5 分钟；退避 1/2/5/10/15 分钟；节流和合并；前台 60 秒；间隔校验 |
| `parity.rs` | 2 | 13 份 fixture 与 TS 逐字段一致；accountId 不会被序列化 |

## 附录 C：与规格的差异

- **项目布局**：Rust core 是独立的 crate `crates/quotadock-core`，Tauri 外壳是 `src-tauri`。这样 `cargo test` 不需要编译 Tauri；职责划分与规格第 10 条一致，只是目录名不同。
- **`SnapshotQuality` 多了 `PendingConfirmation`**：未决候选中的点在确认前不进入分析。
- **事件类型多了 `ResetCreditCountChanged`**（诊断）：用于记录没有关联额度刷新的 credit 变化。
- **`quota_events` 只存已经判定的事件**（`confirmed=1`），待定的候选在 `quota_candidates` 表里。
- **WindowObservationState 的映射**：
  - `StableSchedule` = used=0 且锚点已确认（窗口已初始化但没用过）。
  - `Active` = used>0。
  - 按这个映射，Test D 的预期结果是 Active。
- **新增单实例锁**，防止重复分析同一个 snapshot（规格没有要求）。
