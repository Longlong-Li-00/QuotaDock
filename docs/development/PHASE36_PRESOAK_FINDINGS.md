# QuotaDock Phase 3.6 Pre-Soak Findings

Windows V1 Release Candidate · 2026-10-11 · Windows 11 Home China 10.0.26200 · Codex 0.162.0-alpha.2 · Claude Code 2.1.278

依据标记沿用 Phase 3.5：`[LOCAL TEST]` 本机实测或自动化测试，`[REAL RUN]` 真实运行（release exe、真实账号），`[QUOTADOCK HEURISTIC]` QuotaDock 自定的规则，`[UNKNOWN]` 未知。

## 结论速览

| 项目 | 结果 |
|---|---|
| Feature freeze | ✅ 只做了 hardening、缺陷修复和诊断；新想法记入 [POST_V1_BACKLOG.md](POST_V1_BACKLOG.md) |
| 非正式预跑（Phase 3.5 版本，63.6 h） | ✅ Codex 820/820 次轮询成功，最长间隔 5.0 min；Claude 263/263 次查询成功，0 次 429；真实观察到 1 次 Codex 每周提前刷新和 3 次 Claude 5h 刷新 |
| A/B Claude 主动刷新默认值 | ✅ 未设置 = 开启；明确的设置（包括旧版写入的 `false`）保持不变 |
| §5 数据来源优先级 | ✅ 不论来源，最新的有效观测胜出；比已存观测旧的状态栏记录不再入库 |
| §6 安全保护 | ✅ 加强：成功的外壳中缺少零推理证据字段时，按未知格式处理（fail closed），不再当作 0 |
| §8–§14 MSIX 守卫 | ✅ 打开 SQLite 之前完成验证；在包内自动经 Explorer shell 重新启动；带标记仍在包内则 fail closed。**真实环境发现**：Claude Desktop 的 shell 没有 package identity，但文件写入照样被重定向，因此另加了写入探测 |
| §15–§17 Watchdog | ✅ MONITORING_GAP、数据库锁卡住检测、睡眠 / 唤醒、资源采样，都写入独立于 SQLite 的 `logs\health.jsonl` |
| §18–§19 Shadow estimator | ✅ 只用于诊断：生产值 / 原点以来平均 / 最近 30 min 斜率，存入 `analysis_diagnostics`（migration v6）；不影响任何显示或决策 |
| §20 Claude 预测门槛 | ✅ 保持 5h 30 min、每周 24 h |
| §24–§33 soak 报告 | ✅ `npm run soak:report -- --days 1` 覆盖全部要求的指标，只读 |
| 预跑中发现的缺陷 | **1 个**：Codex 每周刷新后 3 小时就发出 "可能提前耗尽" 通知（误报）。已修复并加回归测试 |
| 测试 | Rust 151（Phase 3.5 结束时 135）、UI 24、TS 参考实现 83，全部通过；`cargo clippy --workspace --all-targets` 零警告 |
| 7 天 soak | ✅ **已开始：2026-10-11 16:30:32**，pid 60332，通过 Explorer 正常启动（包外），在托盘中运行 |

---

## 0. 非正式预跑

Phase 3.5 结束后，最终版本从 2026-10-09 00:38:06 一直运行到 2026-10-11 16:15:58（为了升级才退出），共 **63.6 h**，期间没有任何人工干预 `[REAL RUN]`。

| 项目 | 结果 |
|---|---|
| Codex 轮询 | 820 次，全部成功（1 次 startup、819 次 scheduled）；最长间隔 5.0 min；超过 6 min 的间隔：0 |
| Claude `/usage` | 263 次，全部成功；0 次 429；最长更新间隔 15.2 min（后台节奏） |
| Codex 5h | 1 次 natural rollover；1 次 sliding anchor；1 次 ±1 s 抖动 |
| Codex 每周 | 18 次 ±1 s 抖动（诊断）；4 次 schedule change 中有 3 次在 1–2 个轮询后被 reconciliation 撤销 |
| Codex 每周提前刷新 | 10-11 13:00:39，见下文 |
| Claude 5h | 3 次 `quota_refreshed_observed`（10-09 01:38、10-10 18:01、10-11 13:55），每次都开了新的窗口实例 |
| 通知 | 2 条：1 条正确（每周刷新），1 条误报（每周预测，见 §2） |

### Codex 每周提前刷新（真实事件，检测正确）

```text
#911 12:55:39  weekly 10%  resetsAt 10-14 21:19:44
#912 13:00:39  weekly  0%  resetsAt 10-18 13:00:41   ← now + 7 d（sliding：窗口空闲）
#913 13:05:40  weekly  0%  resetsAt 10-18 13:05:41
#914 13:10:40  weekly  0%  resetsAt 10-18 13:10:42   → 连续 3 次一致后确认
#917 13:25:42  weekly  0%  resetsAt 10-18 13:21:08   ← 开始使用，窗口锚定
```

- 用量从 10% 降到 0%，计划重置时间（10-14）还没到，reset credit 没有变化（3 → 3）。这不是用户兑换 credit。
- QuotaDock 记为一条低置信度的 `unscheduled_quota_refresh`，不推测原因，只发了一条通知：
  - "Codex 额度已刷新 (每周)"
  - "QuotaDock 检测到计划时间之外的额度刷新。原因未知。"
- 之后的 sliding anchor 和锚定都是诊断事件。没有重复通知，也没有假 rollover。

## 1. Hardening 项目

### A/B. Claude 主动刷新默认开启

- `ClaudeSettings.active_refresh` 改为三态 `Option<bool>`：
  - 未设置（`None`，JSON 中不写这个字段）= 开启；
  - `true` / `false` = 用户的明确选择，永远保留。
- 无法区分 "Phase 3.5 默认写入的 `false`" 和 "用户主动关闭"，所以存储中已有的 `false` 一律视为**明确关闭**（§B：不能把用户的 OFF 改成 ON）。本机存储的是 `true`（用户在 Phase 3.5 中开启），保持开启。
- 保存其他设置时，未设置的值仍然是未设置。
- 设置页开关显示有效值，并增加说明 "默认开启"。
- 节奏不变：只作为数据过期时的补充；新鲜的状态栏数据会跳过主动查询。
- 测试：`active_refresh_defaults_on_but_keeps_an_explicit_choice`。

### §5. 最新的有效观测胜出

- `store::latest` 本来就按观测时间取最新，不看来源。
- 补上一个缺口：以前被动源只和 "上一条状态栏记录" 比较，所以比最近一次主动读取更旧的状态栏记录也会入库（乱序）。现在和**任何来源**的最新观测比较。
- 测试：`the_newest_observation_wins_whatever_its_source`。

### §6. 主动查询的安全保护

- 原来的检查：`local_command == "usage"`；`duration_api_ms`、`num_turns`、`total_cost_usd`、四类 token 都为 0；`modelUsage` 为空。
- **加强**：对于成功的外壳，`duration_api_ms`、`num_turns`、`usage.input_tokens`、`usage.output_tokens` 必须**存在**并且为 0，`modelUsage` 必须存在（空对象）。缺少任何一项 → `Unrecognized` → 本次运行停用主动源（needs_verification），不再查询。
- 这样，将来 Claude Code 版本改了外壳格式时，不会因为 "字段缺失被当成 0" 而静默通过（§39 "parser silently accepts incompatible output"）。
- 测试：`missing_zero_inference_evidence_is_not_read_as_zero`。

### §7. 限流

不变，已在 Phase 3.5 实现并测试：先用 Retry-After，否则 15 / 30 / 60 min；连续 3 次 429 后，本次运行暂停主动源；被动源不受影响；provider 不会被标成失败。

### §8–§14. MSIX 虚拟化守卫

**设计**（`crates/quotadock-core/src/platform/data_location.rs`）：

```text
main() 第一件事（Tauri、SQLite、日志都还没开始）
  PackageContext::current()
    1. GetCurrentPackageFamilyName       → ClaudeDesktop(family) / Other(family)
    2. 没有 identity 时做写入探测：在 %LOCALAPPDATA% 下建一个唯一的空文件，
       看它是否出现在某个 Packages\<family>\LocalCache\Local 下，然后删除
                                          → Redirected(family) / Unpackaged
  startup_decision(context, args)
    Unpackaged                 → Proceed（%LOCALAPPDATA%\QuotaDock，或 QUOTADOCK_DATA_DIR）
    在包内，没有标记           → Relaunch：只保留 --hidden / --autostart，加上 --quotadock-unpackaged-relaunch
    在包内，带标记             → FailClosed：双语提示，退出，不打开 SQLite
```

- **重新启动的方式**：通过 Explorer shell 自己的进程执行 ShellExecute（桌面文件夹视图的 `Shell.Application`）。
  - 新进程的父进程是 explorer，不在任何包内。
  - 可执行文件路径来自 `current_exe()`。
  - 参数在 `startup_decision` 中按白名单过滤，`relaunch_via_shell` 再检查一次。
- **不变式**：
  - 原来的 `storage::default_database_path()` 已删除。规范路径只能通过 `validated_database_path()` / `decide_current()` 取得，二者都先做包检查，而且只解析路径，不创建任何东西。
  - 开发工具 `quotadock-dev` 不传 `--db` 时也走同一个检查。
  - 测试：
    - `unpackaged_uses_the_canonical_folder_and_creates_nothing`
    - `packaged_or_virtualized_folders_are_refused`
    - `a_packaged_start_relaunches_once_with_allow_listed_arguments`
    - `a_probe_found_in_a_package_store_names_that_package`
    - `the_shell_relaunch_refuses_unknown_arguments`
- **不合并数据库**（§13）：包私有存储中的旧数据（只剩 Phase 3 时的 WebView2 目录 `com.quotadock.app`）不复制、不移动、不删除。

**真实环境发现** `[LOCAL TEST]`：

| 检查 | 结果 |
|---|---|
| 本开发会话的 shell（Claude Desktop Code 标签）调用 `GetCurrentPackageFamilyName` | **15700（APPMODEL_ERROR_NO_PACKAGE）**：没有 package identity |
| 同一个 shell 在 `%LOCALAPPDATA%` 下新建文件 | **实际写到了 `Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\`** |
| 从这个 shell 启动的旧版本 QuotaDock 写入已存在的 `%LOCALAPPDATA%\QuotaDock` | WAL、SHM 和新建的 `logs\health.jsonl` 都写到了真实位置 |

也就是说：

- 只靠 package API **检测不到** Phase 3.5 实际遇到的情况。
- 重定向作用于 Claude Desktop 进程树中的 "新建项目"。真实的 `QuotaDock` 文件夹已经存在时，里面的写入目前不受影响；但在全新安装时（文件夹不存在），整个数据目录都会被建进包存储，正是 Phase 3 的情况。

所以守卫以 package identity 为先，再用写入探测作为直接证据。这一点与 §9 "优先使用 Windows package APIs" 的顺序一致，但单靠它不够，需要 GPT 知悉。

**真实验证** `[REAL RUN]`：

1. 从被重定向的 shell 运行 `quotadock-dev data-location`：`redirected_by_claude_desktop Claude_pzs8sxrjxfjjc → Relaunch`。从 Explorer 启动的批处理运行：`unpackaged → Proceed %LOCALAPPDATA%\QuotaDock`。探测文件没有残留。
2. 从被重定向的 shell 直接运行 `quotadock-app.exe --hidden`：
   - 子进程以代码 0 退出；
   - 新实例的父进程是 **explorer**，命令行是 `--hidden --quotadock-unpackaged-relaunch`；
   - health log 中有 `app_start launch=hidden relaunched=true`，窗口保持隐藏；
   - 包存储中没有出现 QuotaDock 目录。
3. 从被重定向的 shell 运行带标记的 `quotadock-app.exe --quotadock-unpackaged-relaunch`：
   - 显示 §12 规定的双语提示（截图 `phase36-failclosed.png`，只截了这个对话框）；
   - 点 "确定" 后退出；
   - health log 中没有这个进程的任何记录，也就是说它没有打开数据目录。

### §15–§17. Watchdog 与健康日志

- **健康日志**：`%LOCALAPPDATA%\QuotaDock\logs\health.jsonl`（JSON lines，4 MB 轮转）。它不经过 SQLite，所以数据库卡住时也能写。

  | kind | 内容 |
  |---|---|
  | `app_start` / `app_stop` | 版本、pid、启动方式（normal / hidden / autostart）、是否经过重新启动 |
  | `foreground` / `paused` | 窗口显示 / 隐藏和暂停状态的变化，用来解释节奏 |
  | `monitoring_gap` | MONITORING_GAP 打开和关闭（provider、开始时间、时长、阈值、连续失败次数、是否有读取在进行） |
  | `db_busy` / `db_busy_end` | 连续 3 次每分钟检查都发现数据库 mutex 被占用 |
  | `power` / `power_watch` | 系统睡眠 / 唤醒（`PowerRegisterSuspendResumeNotification`）。soak 实例的注册结果：`registered: true` |
  | `resources` | 每 30 min 一次：QuotaDock、它的 WebView2 进程树、codex app-server、claude 子进程的工作集、私有字节、句柄、CPU 时间；以及 "孤儿候选"（父进程已不存在的 codex.exe / claude.exe） |
  | `claude_env` / `claude_active` | Claude Code 版本或认证方式变化；主动源因 429 暂停，或进入 needs_verification |

- **Watchdog**：每分钟在托盘计时器中运行，**不拿数据库锁**。
  - Codex：从轮询控制器的内存状态读取最后一次读取的开始时间。
    - 前台超过 3 × 60 s、后台超过 2 × 300 s 记为 MONITORING_GAP。
    - 失败退避时，用计划的退避间隔作为期望值，不算 gap。
    - 一次读取卡住不返回，也会表现为 gap，并标注 `readInFlight`。
  - Claude：只在主动刷新可用、没有暂停、不在退避中时检查，阈值为 2 × 5 / 15 min。
  - 这些是 `[QUOTADOCK HEURISTIC]`，不是额度事件，也不会通知用户。
- **为此在轮询状态中增加**：最后一次读取开始 / 结束的墙钟时间。另外增加 `QuotaDockCore::db_busy()`（`try_lock`，从不阻塞）。
- **测试**：
  - `lifecycle_pause_and_window_changes_are_logged_next_to_the_database`
  - `the_watchdog_samples_resources_and_reports_a_stuck_database`：另一个线程持有数据库锁，watchdog 记录 `db_busy`，释放后记录 `db_busy_end`
  - `a_gap_opens_once_and_closes_when_activity_resumes`
  - `records_are_json_lines_with_time_and_kind`
  - `process_trees_are_followed_through_children`
- **soak 开始时的第一个资源样本**（托盘中）：

  | 进程 | 资源 |
  |---|---|
  | QuotaDock | 44–46 MB 工作集，15 MB 私有，约 395 个句柄 |
  | WebView2 | 6 个进程，408–428 MB |
  | codex app-server | 61–67 MB |
  | 孤儿候选 | 0 / 0 |

### §18–§19. Shadow estimator（只用于诊断）

- 生产算法（detector-v2 重锚、过原点最小二乘）**没有改动**。
- 每次 Codex 轮询后，如果 5h 分析段有派生起点，就记录三个值到 `analysis_diagnostics`：
  - A：生产值；
  - B：原点以来的平均速度；
  - C：最近 30 分钟的斜率，至少 3 个点、跨度 20 min 才计算。
- 每次新的重锚另记一行 `reanchor`。
- migration v6 只新建这张表，不改动任何已有的表。
- 这些行只出现在诊断导出和 soak 报告中，Dashboard、风险、托盘、通知和预测都**不读取**它们。
- 第一条真实记录 `[REAL RUN]`：起点 10-11 13:21:08（derived），第一个有用量的快照 #1006；A 11.2–11.9 pp/h，B 13.0–13.4 pp/h，C 数据不足。
- 测试 `shadow_estimates_compare_the_production_fit_with_simpler_rates` 使用 Phase 3.5 的真实前置用量数字：A 明显高于 B，C = 0。

### §20. Claude 预测门槛

保持 5h 30 min、每周 24 h。每周的门槛现在由通用的 `forecast` 执行（见 §2），所以 Claude 每周预测也会像 Codex 一样，在有超过一天的数据时使用 "最近 24 h" 的速度。

### §24–§33. soak 报告

`scripts/soak-report.mjs` 重写（只读：数据库以只读方式打开，health log 只读，System 事件日志只查询）：

| 部分 | 内容 |
|---|---|
| 数据库 | `PRAGMA integrity_check`，数据库 / WAL / SHM 大小，各表行数 |
| 运行时长 | 启动和退出次数、启动方式、不干净退出、机器开机 / 关机 / 睡眠（System 事件日志 12/13/41/42/107/6008）、QuotaDock 收到的电源事件、实际监控覆盖率 |
| Codex | 成功 / 失败 / 成功率；按触发方式；前 10 个最长间隔，每个都附开始、结束、期望节奏、窗口状态、是否暂停、原因（暂停 / 机器睡眠 / 应用未运行 / provider 错误 / 正常 / unknown）；provider 启动和意外退出；版本变化；5h 和每周的 rollover、sliding、重锚、抖动、schedule change；ordinaryUsageAllowed 变化；缺失窗口；ResetCreditRedeemed / UnscheduledQuotaRefresh；每个额度边界的前后数值 |
| resetsAt 稳定性 | Codex 的 0 s / ±1 s / ±2 s / 3–10 s / >10 s 分布，空闲的 sliding 和真实额度边界单独统计；Claude 按来源统计，`/usage` 的 60 s 跳变单独标为分钟取整 |
| Claude | 版本和认证变化；状态栏 / 主动查询的来源占比 %；查询次数、成功、错误、429、needs_verification、暂停；耗时；前 10 个最长更新间隔；过期时段；5h / 每周实例数；额外额度观测数；刷新事件 |
| 通知审计 | 按 provider 和规则统计；跨 provider 合并数；**重复的 规则+scope**；最近 10 条 |
| Shadow estimator | 每次重锚的 A / B / C 和 A÷B 的范围 |
| Watchdog | MONITORING_GAP 打开 / 关闭 / 仍未关闭，数据库锁卡住次数 |
| 资源 | 首次 / 最后 / 最小 / 最大值和每日趋势（线性回归）；WebView2 在窗口打开和托盘中的中位数；隐藏和显示时的平均 CPU；孤儿候选 |

在真实数据库上运行了 4 天范围：所有超过阈值的历史间隔都被归类为 "应用未运行"（Phase 3.5 开发期间的重启），**unexplained 为 0**；重复通知为 0。

## 2. 预跑中发现并修复的缺陷

| | |
|---|---|
| **现象** `[REAL RUN]` | 10-11 16:10:25 发出 "Codex 额度可能提前耗尽 (每周)" |
| **证据** | 每周窗口 13:00 提前刷新，13:21 锚定，到 15:35 一直是 0%，16:05 3%，16:20 6%。一次工作会话约 3 小时的速度（约 48 pp/天）被外推到整周 → "约 2 天耗尽" → AtRisk → 通知 |
| **影响** | 每次每周 rollover 或刷新之后的头几个小时都会这样。下一次 Codex 每周重置（10-18）就在 soak 期间 |
| **修复** | 每周预测（风险、速度、通知、托盘）在分析段内有 **24 h** 观察之前报告 "历史数据不足"。门槛只限制结果，**速率算法不变**（周中仍然优先使用最近 24 h 的速度）。Codex 和 Claude 使用同一个门槛（`AnalyticsConfig.weekly_min_span_ms`，`[QUOTADOCK HEURISTIC]`） |
| **回归测试** | `a_weekly_forecast_needs_a_day_of_observation`（真实数字）；原有的 `weekly_reports_per_day_and_prefers_recent_24h` 继续通过，说明周中行为不变 |
| **验证** | soak 版本的 Overview：Codex 每周显示 "历史数据不足 · 至少需要 24 小时的观察"；Claude 每周（已有 66 h 数据）正常显示 1.1 pp/天、低 |

这属于 §3 允许的 "false notification fix"，没有更换生产估算方法（§18）。但它确实改变了每周预测**何时**给出结果，需要 GPT 知悉。

## 3. 测试

| 套件 | 数量 |
|---|---|
| `cargo test --workspace` | **151** 通过（单元 34、`claude_provider` 24、`engine_scenarios` 25、`product` 21、`storage` 12、`rpc_and_provider` 9、`analytics` 8、`polling` 7、`detector_v2` 6、`soak_hardening` 3、`parity` 2） |
| `cargo clippy --workspace --all-targets` | 零警告 |
| UI（`ui/`，`npm test`） | 24 通过；`tsc --noEmit` 通过 |
| TS 参考实现（`npm test`） | 83 通过 |

新增测试 16 个：数据位置 / 探测 6、健康日志与 watchdog 5、设置三态 1、外壳字段 1、来源优先级 1、每周门槛 1、shadow estimator 1。

## 4. 7 天 soak

| 项目 | 值 |
|---|---|
| 开始 | **2026-10-11 16:30:32**（KST） |
| 最早结束 | 2026-10-18 16:30（≥ 7 天连续） |
| 版本 | `target\release\quotadock-app.exe`（2026-10-11 构建，classifier reset-detector-v2，schema v6） |
| 启动方式 | Explorer 正常启动（父进程 explorer，不在任何包内），在托盘中运行 |
| 配置 | Claude 状态栏桥接 + 主动刷新（用户在 Phase 3.5 的选择）；通知默认值 |
| 每日报告 | `npm run soak:report -- --days 1`（必须在包外运行，见下方说明）；最终用 `--days 7` |

soak 期间预计会出现的边界：

- Claude 每周重置（10-12 23:59 / 10-13 00:00）；
- 多次 Claude 5h 刷新；
- Codex 5h rollover 和 idle→active 重锚；
- **Codex 每周重置（10-18 13:21）**：soak 最后一天，正好可以验证缺陷修复。

注意：

- soak 报告和 QuotaDock 一样，必须在包外运行。从 Claude Desktop 的终端运行 `node` 时，它读到的 `%LOCALAPPDATA%` 视图可能不同。
- 在资源管理器、普通 Windows 终端（不是 Desktop Code 标签）或 Windows 的 "运行" 中执行都可以。
- 报告本身是只读的。
- 机器重启、睡眠、关机不算失败；报告会把它们和应用故障分开（§22）。
- "Windows 启动时运行" 目前**没有开启**（这是用户的设置）。开启后，重启不会造成长时间的 "应用未运行" 间隔。

## 5. 与规格的差异 / 需要 GPT 知悉

| 规格 | 实际 | 原因 |
|---|---|---|
| §9 "优先使用 Windows package APIs" | package API 优先，另加写入探测 | 真实环境中，Claude Desktop 的 shell 没有 package identity，文件写入照样被重定向（§1 证据） |
| §11 "可以通过 Windows Explorer shell 启动自身" | 通过 Explorer 进程中的 `Shell.Application.ShellExecute` | 直接运行 `explorer.exe <exe>` 无法传递 `--hidden` / `--autostart` 和标记参数 |
| §16 记录 MONITORING_GAP | 写入 health log（JSONL），不写数据库 | 数据库卡住时也要能记录 |
| §18 analytics freeze | 每周预测增加 24 h 观察门槛（Codex 和 Claude） | 真实误报通知（§2）；估算方法本身没有改动 |
| §B 区分 unset / disabled | 已存储的 `false` 都视为明确关闭 | 旧版本会把默认值写入存储，无法区分。按 "不覆盖用户的 OFF" 处理 |

## 6. soak 开始时的已知限制

1. 主要使用 Claude Desktop 时，Claude 数据全部来自主动刷新（Desktop Code 标签不运行 statusLine）。soak 的来源占比会给出数字（§27）。
2. 通知的身份仍然是开发版的 PowerShell AUMID；正式身份在打包阶段解决（§47）。
3. 启动后约 0.8 s 内收到 WM_CLOSE 会让进程退出（Phase 3.5 已记录；用户无法触发）。
4. Watchdog 的检查间隔是 1 分钟，比这更短的卡顿检测不到；"UI 冻结" 本身没有专门的检测。托盘计时器和 watchdog 在同一个任务中运行，如果 tokio 运行时整体卡住，watchdog 也无法记录，只能从 health log 中的时间断层看出来。
5. 包私有存储中 Phase 3 时期的 WebView2 目录（`com.quotadock.app`）按 §13 保留不动。
