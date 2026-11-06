# QuotaDock Phase 3 UI Findings

QuotaDock Phase 3 · 2026-10-08 · Windows 11 · Rust 1.99.0（stable-msvc）· Tauri 2.12.1 · React 19.3 · Vite 8.3 · TypeScript 7.0.2

依据标记沿用 Phase 2：

| 标记 | 含义 |
|---|---|
| `[LOCAL TEST]` | 本机实测，或本仓库的自动化测试 |
| `[REAL RUN]` | 本次 Phase 3 真实运行：release exe，连接本机真实账号 |
| `[OPENAI CODEX SOURCE]` | openai/codex 源码（tag `rust-v0.162.0-alpha.2`） |
| `[TAURI SOURCE]` | 本机 cargo registry 中的 Tauri / 插件源码 |
| `[QUOTADOCK HEURISTIC]` | QuotaDock 自己定的规则或阈值，不是 OpenAI 的行为或指标 |
| `[INFERENCE]` | 推断 |
| `[UNKNOWN]` | 未知 |

## 结论速览

| 验收项（§75 / §77） | 结果 |
|---|---|
| Overview / History / Events / Settings | ✅ 四个页面都已实现，真实窗口里渲染的是真实数据 `[REAL RUN]` |
| System Tray | ✅ 图标随 severity 和暂停状态变化；tooltip 和菜单已本地化；左键单击打开窗口 `[REAL RUN]` |
| Pause / Resume | ✅ 暂停时不再轮询，QuotaDock 的 app-server 也会退出；恢复时立即读取一次；暂停状态不跨重启 `[REAL RUN]` `[LOCAL TEST]` |
| Windows Notifications | ✅ 中文和英文的 toast 都已进入通知中心 `[REAL RUN]`。未安装的 exe 用的是 PowerShell 身份，见 Known limitations |
| Notification Dedup | ✅ 按周期持久化去重，重启后仍有效，新周期会重新武装 `[LOCAL TEST]`。真实运行中没有出现触发条件（weekly 94–95%），所以写入 0 条，也没有误报 |
| Single Instance | ✅ 第二次启动会把参数转给第一个实例后以 code 0 退出；第一个实例的窗口被显示 `[REAL RUN]` |
| 中文 / English | ✅ System → English → 中文 → System 的切换都无需重启，页面、托盘和通知同步生效 `[REAL RUN]` |
| Light / Dark / System | ✅ 用 CSS 变量加 `data-theme`；原生标题栏通过 `set_theme` 同步。真实窗口中切换 Dark 和 System 都即时生效 `[REAL RUN]` |
| §77 安全验收 | ✅ webview 只能调用 15 个窄命令；没有 shell、SQL、文件系统、RPC、通知、托盘或 core 权限；没有 token、OAuth、login/logout 或 reset credit consume |
| 真实运行（§70） | ✅ 最终二进制连续运行 120.3 分钟（12:05:51–14:06:00），28 次 poll 全部成功：前台 60–61 s，后台 300–302 s；退出后没有遗留 codex.exe。从 11:00 起全部 5 段共 89 次 poll，0 失败 `[REAL RUN]` |
| 测试 | Rust 81 个、UI 20 个、TS 参考实现 83 个，全部通过；clippy 零警告 |
| 真实运行中发现并修复的缺陷 | 5 个：退出死锁、关闭提示残留、provider error 误报、隐藏时页面定时器仍在运行、soak 日志缺口。另有 1 个按 §73 只留证据、暂不修改，见 §Real-world test |

---

## 0. 开工前 Review（§74）

1. **Tauri placeholder 结构**：Phase 2 的 `src-tauri` 只有一个静态占位页和 6 个命令，没有任何插件；polling 在 `setup` 中启动。
2. **原有 commands**：`get_current_usage`、`get_usage_history`、`get_quota_events`、`get_usage_forecast`、`refresh_usage`、`get_provider_status`。
3. **新增 commands**：`get_dashboard`、`pause_monitoring`、`resume_monitoring`、`get_settings`、`update_settings`、`acknowledge_close_hint`、`open_data_folder`、`export_diagnostics`、`quit_app`。共 15 个，见 §Security review。
4. **System tray**：使用 Tauri 2 内置的 `TrayIconBuilder`（feature `tray-icon`），图标是嵌入二进制的 5 个 PNG，状态模型来自 Rust。
5. **Notification**：使用 `tauri-plugin-notification`，**只从 Rust 调用**，webview 没有任何权限。
6. **Single instance**：使用 `tauri-plugin-single-instance`，必须是第一个注册的插件；数据库层另有 `quotadock.lock` 文件锁（Phase 2）作为第二道防线。
7. **Notification persistence**：新增 migration v2，含 `notification_state`（去重）和 `notification_log`（审计）。
8. **Bilingual strings**：托盘和通知的文案在 Rust 的 `product/i18n.rs`，页面文案在 TypeScript 字典里，`zhCN: Dict` 在编译期保证两边 key 一致。
9. **Tray severity model**：放在 core（`product/tray.rs`），是纯函数，只依赖 SQLite 中的事实，不依赖 UI。
10. **Pause/Resume 与 PollingController**：controller 新增 `Pause(ack)` 和 `Resume` 命令。`pause()` 等正在进行的读取结束后才返回，返回后再停 app-server。
11. **Notification aggregation**：每次 poll 先收集候选通知，再按优先级合并成最多一条，见 §Notification aggregation。
12. **SQLite migration**：需要。v2 是 `phase3_notifications`；真实运行后又加了 v3 `phase3_provider_sessions`，满足 §72 的 soak 日志要求。

## Architecture

```text
 ┌──────────────────────── Tauri webview (React, ui/dist) ────────────────────────┐
 │ Overview · History · Events · Settings · close hint                             │
 │ Formats and renders DTOs only. No thresholds, reset detection or forecast here. │
 └───────────────▲─────────────────────────────────────────────────────────────────┘
                 │ 15 typed commands (capabilities/default.json) + 4 fixed eval hooks
 ┌───────────────┴────────────── src-tauri (quotadock-app) ────────────────────────┐
 │ commands.rs  tray.rs  notify.rs  window.rs  main.rs (single-instance, args, exit)│
 └───────────────▲─────────────────────────────────────────────────────────────────┘
                 │
 ┌───────────────┴────────────── crates/quotadock-core ────────────────────────────┐
 │ product/   runtime (pause, after_poll, dashboard, diagnostics export)            │
 │            facts → tray (severity) · notifications (rules, aggregation, dedup)   │
 │            history (series, segments, markers) · settings · i18n                 │
 │ polling/   controller: 60 s foreground / 5 min background, backoff, coalescing,  │
 │            Pause/Resume                                                          │
 │ pipeline   provider → normalize → SQLite → reset-detector-v1 (unchanged)         │
 │ codex/     one persistent `codex app-server` (Job Object), 3-method allowlist    │
 └──────────────────────────────────────────────────────────────────────────────────┘
```

一次 poll 的完整流程：

1. `PollingController` 调用 `poll_once`。
2. pipeline 写入 snapshot 并运行 reset-detector-v1（Phase 2 原样）。
3. `ProductRuntime::after_poll` 从 SQLite 重新读取 `MonitorFacts`。
4. 用 facts 算出托盘模型，再评估通知规则、聚合、写去重状态和日志。
5. `tray::apply` 更新托盘，`notify::deliver` 发送 toast。
6. `window::refresh_ui` 让页面重新读取本地数据。

托盘 severity 和通知规则都**只依赖 SQLite**，所以重启前后行为一致：去重状态、"上一个权威 availability"、失败连续段都能从数据库中恢复。

## Frontend/backend boundary

| 层 | 负责 | 不负责 |
|---|---|---|
| Rust core | availability、reset schedule 分类、ObservedCycle、burn rate、forecast、risk、托盘 severity 和原因码、通知规则、去重、聚合、history 分段和降采样 | — |
| Tauri shell | 托盘、toast、窗口显隐、前台检测、单实例、命令参数、自启动 | 业务判断 |
| React | 排版、本地化、日期格式、倒计时文字（只针对 Rust 标为 `confirmed` 且时间在未来的 resetAt）、图表绘制 | 阈值、reset 检测、forecast、severity |

几个边界上的细节：

- **用量条颜色**：取自 Rust 托盘模型的原因码（`weekly_remaining_10`、`five_hour_remaining_30` 等）。前端**没有** 10%/30% 常量。早期版本前端自己算过颜色，已在开发中移除（§61）。
- **"就绪"徽标**：Rust 把窗口标为 `idle_or_uninitialized` 时才显示。
- **sliding / unconfirmed / passed**：前端只是把 Rust 给出的 `resetSchedule` 字符串映射成文案。
- **页面刷新**：后端通过 `window.__quotadock.refresh()` 通知页面重新读取本地数据，不需要给 webview 事件权限。窗口显示时，页面每 5 秒读一次本地 SQLite（**从不**触发后端请求），并有 1 秒时钟。窗口隐藏或最小化时，后端调用 `setActive(false)`，页面停止所有定时器；poll 之后仍会通过 `refresh()` 更新一次数据。

## Screens

所有截图都只截 QuotaDock 自己的窗口（DPI-aware PrintWindow），不包含其他窗口的内容。

- **Overview**：
  - 每个 provider 一张卡片，显示 availability 徽标、5h 和 weekly 剩余百分比、用量条、已用百分比、reset 文案（附 InfoTip）。
  - 右侧是 Forecast 卡片：消耗速度、预计耗尽、重置时间、margin、额度压力、风险，并标注 "QuotaDock 估算"。
  - 异常状态会显示 HealthBanner：连接中、未找到 Codex、未登录、版本不受支持、重连中、已暂停。
  - 真实窗口实测过以下状态 `[REAL RUN]`：
    - 中文空闲态：就绪、100% 剩余、恢复使用后确认计时，每周 98%；
    - 英文暂停态：Monitoring paused 横幅，Refresh 按钮禁用，按钮文字为 Resume monitoring；
    - 英文活跃态：5h 82% 剩余、"Resets in 4h 37m"，weekly 95%，以及 forecast 数值。
- **History**：
  - 范围有 24h、7d、30d 和当前周期，可切换显示 "剩余" 或 "已用"。
  - 图表是自绘 SVG；下方显示 burn rate 并标注为估算。
- **Events**：
  - 默认只显示用户可见事件；打开 "显示诊断事件" 后加入诊断事件。
  - 每页 50 条，"加载更多" 使用 keyset 分页（`before_id`）。
  - 每条事件可以展开，显示前后数值、证据和置信度。诊断模式下可以复制技术细节。
- **Settings**：
  - 通知开关 9 个，默认值见 §Notification categories；
  - 外观：语言、主题；
  - 启动与窗口：开机启动、关闭到托盘；
  - 诊断：显示诊断事件、打开数据文件夹、导出诊断；
  - 退出 QuotaDock。
- **关闭提示**：
  - 本次会话第一次点 X 时弹出模态框："QuotaDock 仍在运行 / 会继续在系统托盘中运行"，附 "不再提示"。
  - 点确定后窗口隐藏。

## Tray behavior

- **菜单**：打开 QuotaDock、立即刷新、（分隔线）、Codex、5h 剩余、Weekly 剩余、（分隔线）、暂停/恢复监控、退出。
  - Codex、5h、Weekly 三项是禁用的信息行。
  - 暂停时 "立即刷新" 禁用，"暂停" 文字变为 "恢复"。
- **左键单击**打开主窗口，右键弹出菜单（`show_menu_on_left_click(false)`）。
- **Tooltip** 最多 127 个字符。实测内容：`QuotaDock / 5h: 100% · Weekly: 98% / Updated just now`，切换语言后立即变成英文 `[REAL RUN]`。
  - 60 秒的本地 ticker 负责刷新 "N 分钟前更新" 和过期状态（只读 SQLite）。
- **图标**有 5 种：normal、warning、critical、error、paused。
  - 每种除颜色外还有不同的形状（圆环、!、!!、×、暂停双竖线），不只靠颜色表达（§68）。
- **关闭到托盘**（默认开启）：
  - X 触发 `CloseRequested`，被 `prevent_close` 拦截。
  - 本次会话第一次关闭、且用户没有勾选 "不再提示" 时，先显示提示；否则直接隐藏。
  - 只有托盘的 "退出"、设置页的 "退出 QuotaDock" 和 `--quit` 能真正退出。
  - 关闭 "关闭到托盘" 后，X 就是退出。
- **托盘更新的线程模型**：Tauri 托盘和菜单的 setter 从非主线程调用时，会同步等待主线程执行完毕 `[TAURI SOURCE]`。所以 `tray::apply` 把更新 post 到主线程后立即返回，原因见 §Real-world test 的 "退出死锁"。

## Tray severity rules

实现在 `product/tray.rs::derive_tray_status`，是纯函数。优先级 **error > critical > warning > normal**。暂停时图标固定为 paused，但 severity 和原因码照常计算。

| Severity | 条件 | 原因码 |
|---|---|---|
| error | 最近一次 poll 失败，且状态是 `authentication_error` | `not_authenticated` |
| error | 最近一次 poll 失败，且状态是 `provider_unavailable`（未找到 Codex 或版本门禁） | `provider_unavailable` |
| error | 未暂停，且最近有效 snapshot 超过 30 分钟 | `no_valid_snapshot_30m` |
| critical | `ordinaryUsageAllowed == false` | `limit_reached` |
| critical | weekly 剩余 ≤ 10%（weekly 提醒开启时） | `weekly_remaining_10` |
| critical | 5h 剩余 ≤ 10%（**仅当** 5h 提醒开启时） | `five_hour_remaining_10` |
| warning | weekly 剩余 ≤ 30% 且 > 10%（weekly 提醒开启时） | `weekly_remaining_30` |
| warning | 5h 剩余 ≤ 30% 且 > 10%（仅当 5h 提醒开启时） | `five_hour_remaining_30` |
| warning | weekly forecast 为 AtRisk | `weekly_forecast_at_risk` |
| warning | availability 连续 3 个成功 snapshot 都是 null | `availability_unknown_persistent` |

补充说明：

- 剩余百分比只在窗口存在、且该窗口质量为 `normal` 时参与判断。pending、suspect 或 missing 的窗口不会触发阈值。
- 30% 原因只在剩余 > 10% 时出现，所以同一个窗口不会同时列出 10% 和 30% 两条原因。
- 新鲜度 `[QUOTADOCK HEURISTIC]`：≤10 分钟为 fresh；10–30 分钟为 maybe stale，只改变文案；超过 30 分钟为 stale，即 error。

测试覆盖：

- `tray_severity_precedence`
- `tray_unknown_availability_becomes_warning_after_three_snapshots`
- `tray_five_hour_only_counts_when_enabled`：5h 剩余 5%、5h 提醒关闭时，severity 仍为 normal
- `tray_paused_and_stale_presentation`

## Pause/resume

- **入口**：顶栏按钮、托盘菜单、`--pause` / `--resume`。
- **`ProductRuntime::pause()` 的顺序**：
  1. 先调用 `polling.pause()`，等正在进行的读取结束后返回。
  2. 再调用 `core.shutdown()` 停止 QuotaDock 自己的 app-server 子进程。
  3. 然后更新托盘，图标变为 paused，"立即刷新" 禁用。
- **暂停期间**：
  - 控制循环只响应命令，不调度任何读取。
  - `refresh` 返回 `Paused`。
  - 托盘的 30 分钟过期 error 被抑制，因为数据旧是用户主动暂停造成的。
- **`resume()`**：controller 用 `Manual` trigger 立即 poll 一次。provider 按需重新启动 app-server，`provider_sessions` 记一条 `after_stop`。
- **不持久化**：暂停状态只在内存里（`AtomicBool`），重启后一定恢复监控；没有定时恢复功能。
- **实测** `[REAL RUN]`：
  - 11:11:26 暂停后，app-server 子进程立即退出；暂停期间 `--refresh` 无效；约 2.5 分钟内 0 次 poll（原本应有 2–3 次 60 秒 poll）。
  - 11:14:02 恢复，11:14:03 立即出现 manual poll，app-server 以新 pid 启动。
- **测试**：
  - `pause_stops_scheduling_and_resume_polls_immediately`（polling）
  - `pause_stops_the_app_server_and_is_not_persisted`（product）：用 fake app-server 验证子进程退出、暂停时 refresh 返回 Paused、托盘为 paused、恢复后重新启动 provider 并记录 `initial` 和 `after_stop`、重启后不再处于暂停

## Polling integration

- **间隔**：前台 60 秒，后台 5 分钟（Phase 2 的 controller）。
- **前台的定义**：主窗口可见且未最小化。`window.rs::sync_foreground` 在 Focused、Resized、显示、隐藏时调用，只在状态变化时通知 controller。切换后，下次 poll 时间为 `上次开始时间 + 新间隔`。
- **手动刷新**：两次手动 poll 至少间隔 10 秒，否则返回 `Throttled`；读取中到达的请求返回 `Coalesced`，由正在进行的读取服务。
- **失败**：指数退避（Phase 2）。
- **启动**：启动时立即 poll 一次。`--autostart` 和 `--hidden` 启动不显示窗口，从后台间隔开始。
- **实测** `[REAL RUN]`：前台间隔稳定在 60–61 秒；隐藏窗口后切到 5 分钟（数据见 §Real-world test）；`--refresh` 紧接 resume 时被节流，30 秒后再发则正常执行。

## Notification categories

所有通知都**只从 Rust 发出**。下表中的规则和阈值都是 `[QUOTADOCK HEURISTIC]`。

| 规则 id | 触发 | 默认 | 去重 scope |
|---|---|---|---|
| `availability_restored` | 权威 availability 从 false 变为 true（见下节） | 开 | `blocked:{上一个 false snapshot id}`，即每个受限期一次 |
| `reset_credit` | `ResetCreditRedeemed`，且只在 HIGH 置信度时 | 开 | event_key |
| `quota_refresh` | `UnscheduledQuotaRefresh`，正文注明 "原因未知" | 开 | event_key |
| `allowance_refresh` | `NaturalRolloverObserved`，通知标题为 "Codex 额度已刷新"（事件列表中是 "5 小时额度已刷新" / "每周额度已刷新"），不说 "新窗口开始" | **关** | event_key |
| `quota_below_10` / `quota_below_30` | 剩余从 >阈值 跌到 ≤阈值；weekly 开，5h 关 | weekly 开、5h 关 | ObservedCycle key |
| `forecast_at_risk` | weekly risk 从非 AtRisk 变为 AtRisk | weekly 开、5h 关 | ObservedCycle key |
| `provider_error` | 连续失败 ≥3 次，或失败连续段已持续 >15 分钟 | 开 | `failures:{连续段首个 poll id}` |

规则细节：

- **诊断事件从不发通知**，例如 `reset_credit_count_changed`、`TransientBackendState`、`SlidingAnchorDetected`。Watch 级别的风险也不发通知。
- **没有 "连接已恢复" 通知**。
- **窗口缺失或 remaining 为 null 时，不触发任何阈值通知**。
- **启动时就 ≤10%**：发一次 10% 通知，同时把同周期的 30% 标记为已发，避免下一次 poll 再补发 30%。
- **测试**：
  - `startup_below_ten_percent_sends_the_ten_percent_alert_once`
  - `five_hour_reminders_are_off_by_default`
  - `missing_window_never_alerts`
  - `diagnostic_events_never_notify_and_credit_needs_high_confidence`
  - `provider_error_needs_three_failures_and_fires_once_per_streak`
  - `provider_error_time_rule_counts_the_failure_streak_not_time_the_app_was_closed`

## Notification aggregation

每次 poll **最多发一条** toast。

- **优先级**（数字越小越高）：availability 1 > credit/refresh 2 > critical quota 3 > forecast 4 > warning quota 5 > provider error 6 > allowance refresh 7。
- **合并同一窗口的低额度和 AtRisk**：同一 poll 中同一窗口同时触发时，合并为一条，标题为 "Codex 额度较低" / "Codex quota is running low"，正文同时包含剩余百分比和风险。
- **其他候选**按优先级排序：第一条作为标题和正文，其余最多 2 条以 "另外：" 行追加。
- **所有被合并的规则**都写入去重状态（包括被并进 "另外：" 的），并在 `notification_log.rule_ids_json` 中留下审计记录。
- **测试**：`low_quota_and_forecast_in_one_poll_become_one_message`、`several_triggers_produce_one_notification_ordered_by_priority`。

## Notification dedup persistence

- **`notification_state` 的主键**是 `(rule_id, provider, window_type, scope_key)`。
  - quota 和 forecast 规则的 scope 是 ObservedCycle key，所以**新周期就是新 scope，自动重新武装**，不需要专门的 "re-arm" 写入。
  - 事件类规则用 event_key 作 scope；受限期和失败连续段各有自己的 episode key。
- **发送前**先查 state，已发过就跳过；发送后 upsert state，并写入 `notification_log`（标题、正文、语言、规则 id、优先级）。
- **toast 交付结果**写回 `delivered` / `error`。注意：插件把 toast 提交给 Windows 时不返回系统结果，所以 `delivered = 1` 的含义是 "已提交给 Windows"，见 Known limitations。
- **测试 `weekly_threshold_dedup_survives_restart_and_rearms_on_a_new_cycle`** 覆盖 §69 的完整序列：
  - 31→29：发 30%；
  - 29→20：不发；
  - 20→9：发 10%；
  - 关闭并重新打开数据库后仍是 9：不重复发；
  - 新周期跌破 30%：重新发。

## Availability recovery logic

- **权威来源**：`ordinaryUsageAllowed` 是唯一来源（§78）。百分比再低也不会被当成 "不可用"。
- **"上一个权威值"**（`MonitorFacts::previous_authoritative`）：同一账号中，最新 snapshot 之前最近一个非 null 的 `ordinaryUsageAllowed`。
- **触发条件**：最新值为 true，上一个权威值为 false，且两者间隔 ≤ 30 分钟 `[QUOTADOCK HEURISTIC]`。
  - **null → true 不通知**。中间夹着 null 时，"上一个权威值" 会跳过 null；如果它是 false，按真实的 false → true 处理。纯 null → true 没有 false 可比较。
  - **启动时为 true 不通知**：QuotaDock 关闭期间的 false → true 间隔超过 30 分钟，视为 "启动时为 true"。这同时满足规格 "startup true → no recovery notification"。例外：如果关闭前是 false，且 30 分钟内重启，启动后的 true 仍按一次恢复处理（同一受限期仍只通知一次）。
  - **true → false 不通知**：托盘变为 critical，Overview 显示 "已达到限制"。
- **去重**：scope 是 `blocked:{上一个 false snapshot 的 id}`，同一受限期只通知一次。
- **测试**：
  - `availability_recovery_false_to_true_notifies_once`
  - `null_to_true_and_startup_true_do_not_notify`
  - `recovery_after_a_long_gap_is_not_treated_as_a_transition`

## Forecast notification logic

- **范围**：只有 weekly，并且只在 risk 从非 AtRisk 变为 AtRisk 时触发，每个 ObservedCycle 最多一次。
  - AtRisk → Watch → AtRisk 在同一周期内不会再发（测试 `forecast_at_risk_once_per_cycle`）。
- **Watch 不发通知**；5h forecast 通知默认关闭。
- **forecast 数据来源**：Phase 2 的 `analytics/forecast.rs`，前端不做任何计算。
  - Phase 3 修正了 margin 的符号：现在正值表示有余量，负值表示可能在 reset 前耗尽，与规格示例一致。
- **托盘**：weekly AtRisk 计为 warning。

## Reset schedule presentation

`resetSchedule` 由 Rust 的 `forecast.rs` 计算，前端只负责把它映射成文案。

| Rust 状态 | 中文 | English | 说明 |
|---|---|---|---|
| `confirmed`，距 reset < 24h | 4 小时 37 分钟后重置 | Resets in 4h 37m | 唯一显示倒计时的情况 |
| `confirmed`，距 reset ≥ 24h | 重置时间 10月14日 21:19 | Reset Oct 14 · 21:19 | 显示日期 |
| `sliding` | 恢复使用后确认计时 | Timing starts after usage resumes | **不显示倒计时**（§78） |
| `unconfirmed` | 正在确认重置时间… | Reset time updating… | |
| `passed` 或 confirmed 但已过时 | 等待新的额度状态 | Waiting for next quota state | |
| 窗口缺失 | 最新数据中未报告 | Not reported in the latest data | 没有阈值提醒 |

- **"就绪" / Ready 徽标**：只在 `idle_or_uninitialized` 时显示。
- **`NaturalRolloverObserved`**：显示为 "5 小时额度已刷新" / "5-hour allowance refreshed"。
- **真实运行中观察到完整的 sliding → confirmed 过程** `[REAL RUN]`：
  1. 11:00–11:10 期间 5h 为 sliding，显示 "恢复使用后确认计时"。
  2. 首次使用后 resetsAt 固定在 16:10:08，界面变为 "Resets in 4h 37m"。
  3. 整个过程没有产生误报事件。

## History chart behavior

- **数据来源**：`product/history.rs::load_history`，只用 `normal` 质量的点。
- **分辨率**：24h 用原始点；7d 按 15 分钟分桶；30d 按 1 小时分桶；当前周期超过 2 天时按 15 分钟分桶，否则用原始点。
- **分段**：ObservedCycle（不含 voided）的 `first_snapshot_id` 是分段边界。
  - 折线在分段边界断开，不会把 reset 前后的数值连成一条斜线。
  - 两点间隔超过 max(45 分钟, 3 × 分辨率) 时标记 `gap_before`，折线也在这里断开。
- **标记**：未被 supersede 的用户可见事件，例如 "额度已刷新"、"额度意外刷新"。
- **Y 轴**：0–100%，可切换显示剩余或已用。时间刻度由纯函数 `timeTicks` 生成（有单元测试）。
- **测试**：Rust `history_series_break_at_cycle_boundaries_and_carry_markers`、`history_downsamples_long_ranges`；UI `chart.test.ts`。

## Events presentation

- **用户可见事件**：额度已刷新（NaturalRolloverObserved）、意外刷新（UnscheduledQuotaRefresh，注明原因未知）、reset credit 已使用（仅 HIGH）、重置时间变化等。
- **诊断事件**：默认隐藏，例如 Transient、SlidingAnchor、`reset_credit_count_changed`、临时缺失。
- **每条事件**：标题已本地化；附带前后数值、置信度徽标、证据行；被 supersede 的事件会标注。诊断模式下还显示技术名并提供 "复制技术细节"。
- **一条真实诊断事件** `[REAL RUN]`：`reset_credit_count_changed`，evidence 为 `{"creditBefore":2,"creditAfter":3,"direction":"increase","cause":"unknown"}`，置信度 low。
  - 这是 QuotaDock 关闭期间 credit 数量增加了。它**不是**使用了 credit，所以不显示在用户事件中，也不发通知，符合规则。
- **全球/促销类 reset 原因从不推断**（§78）：UnscheduledQuotaRefresh 的正文固定为 "原因未知"。

## Bilingual architecture

- **页面文案**：`ui/src/i18n/en.ts` 导出 `en` 和 `type Dict`；`zh-CN.ts` 声明为 `zhCN: Dict`，缺少或多出 key 都会导致编译失败。带参数的文案写成函数，例如 `resetsIn(d)`、`updatedMinutesAgo(n)`。
- **托盘和通知文案**：在 Rust `product/i18n.rs`，`Strings` 结构体分 EN 和 ZH 两个常量。
- **语言偏好**：`system`、`zh-CN`、`en`，默认 System。
  - System 用 `GetUserDefaultUILanguage() & 0x3ff == 0x04`（LANG_CHINESE）判断，中文系统显示中文，其他语言显示英文。
  - Rust 根据偏好得出 `effectiveLanguage` 返回给页面，所以页面和托盘不会各自判断出不同的语言。
- **切换语言**：`update_settings` 保存设置，刷新托盘（tooltip 和菜单），页面用新的 `effectiveLanguage` 重新渲染。通知在发送时按当前语言生成，并在 `notification_log.language` 中记录。
- **实测** `[REAL RUN]`：
  - 中文系统默认显示中文；
  - 用 UI Automation 在真实窗口中把语言切到 English：页面立即变为英文，托盘 tooltip 变为英文，`--test-notification` 发出英文 toast；
  - 再切到中文（同时验证深色主题）、再切回 System：都即时生效；结束时托盘 tooltip 为 "QuotaDock / 5小时：73% · 每周：94% / 刚刚更新"。

## Single-instance behavior

- **插件注册**：`tauri-plugin-single-instance` 是第一个注册的插件。第二次启动时，它把 argv 转发给第一个实例的回调，然后自己以 code 0 退出。
- **第一个实例的处理**（`handle_args`）：
  - 普通启动：显示并聚焦主窗口；
  - `--quit` / `--pause` / `--resume` / `--refresh` / `--test-notification`：只执行对应动作，不弹出窗口；
  - `--autostart` / `--hidden`：不显示窗口。
- **第二道防线**：`QuotaDockCore::open` 用 `share_mode(0)` 锁住 `quotadock.lock`，即使插件失效，两个实例也不可能同时写同一个数据库（Phase 2）。
- **实测** `[REAL RUN]`：
  - 第二次普通启动：第二个进程 code 0 退出，进程数保持 1，原本隐藏的主窗口被显示；
  - 所有 CLI 参数都通过转发生效（`--test-notification`、`--pause`、`--resume`、`--refresh`、`--quit`）。
- **已知细节**：由脚本启动时，Windows 的前台锁可能只显示窗口而不把它切到前台（标题栏为非激活色）。用户双击启动时 `[UNKNOWN]` 未单独验证。

## Security review

**webview 能做的事情只有这 15 个命令**。`capabilities/default.json` 和生成的 `gen/schemas/capabilities.json` 中只有这 15 个 `allow-*`。

| 命令 | 读/写 | 说明 |
|---|---|---|
| `get_dashboard`、`get_current_usage`、`get_usage_forecast`、`get_usage_history`、`get_quota_events`、`get_provider_status`、`get_settings` | 读 | 只读 SQLite 或内存；`get_quota_events` 的 limit 被限制在 1–200 |
| `refresh_usage` | — | 经过 controller 的节流和合并，不会直接访问后端 |
| `pause_monitoring`、`resume_monitoring` | — | |
| `update_settings` | 写 | 参数是强类型的 `AppSettings`（类型不符时 serde 拒绝，未知字段被忽略）；只写 `settings` 表的 `app_settings` 键 |
| `acknowledge_close_hint` | 写 | 只能写一个布尔值 |
| `open_data_folder` | — | **没有参数**，打开 QuotaDock 自己固定的数据目录（`explorer.exe <固定路径>`），不是通用的 shell open |
| `export_diagnostics` | 写 | 没有参数；写到 `<数据目录>/exports/diagnostics-*.json`，内容是脱敏视图，写盘前再整体 sanitize 一次 |
| `quit_app` | — | |

具体检查：

- **插件权限**：Phase 3 引入了 autostart、notification、single-instance 三个插件，但没有给 webview 授予任何插件权限，连 `core:default` 也没有授予。页面不能 listen 事件，也不能操作窗口、托盘或通知。
  - 后端到页面的通知通过 4 个**固定的** JS 调用完成：`window.__quotadock.refresh()` / `showCloseHint()` / `hideCloseHint()` / `setActive(true|false)`。都是常量字符串，不拼接任何数据。
- **CSP**：`default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src ipc: http://ipc.localhost`；`withGlobalTauri: false`。没有远程内容，也没有外链。
- **§77 前端**：没有任意 shell、SQL、文件系统或 JSON-RPC。全仓搜索 `execute_shell`、`execute_sql`、`send_rpc`、`read_arbitrary_file`、`write_arbitrary_file` 都没有命中。
- **§77 QuotaDock 自身**：
  - RPC 白名单仍然只有 `initialize`、`account/read`、`account/rateLimits/read` 和 `initialized`（Phase 2 测试覆盖），所以不会调用 `account/rateLimitResetCredit/consume`、login 或 logout；
  - 不读取 auth.json，不刷新 token，不实现 OAuth；
  - 对 `~/.codex` 0 写入。
- **隐私**：
  - 诊断包只包含规范化视图和 account fingerprint（SHA-256 的前 128 bit），不包含原始 JSON；数据库中的原始 JSON 写盘前，accountId、email 等字段已被替换为 `[REDACTED]`（Phase 2）。日志和 UI 只显示 8 位短 fingerprint；
  - 真实运行中导出的诊断包已扫描，结果见 §Real-world test；
  - V1 没有 telemetry、analytics、崩溃上传或自动更新。
- **开机自启**：`tauri-plugin-autostart` 写 HKCU Run 键，**默认关闭**，只在用户打开设置时写入。本次验证没有打开它。

## Performance

数据来自 `[REAL RUN]` 和构建输出。

- **体积**：
  - release exe 15.2 MB（未打包，前端资源已嵌入）；
  - UI bundle JS 271.7 KB（gzip 84.9 KB），CSS 10.3 KB（gzip 2.8 KB）；
  - 没有 UI 框架以外的运行时依赖，图表为自绘 SVG。
- **单次 poll**：0.39–1.53 秒（含 RPC、SQLite 两个事务、规则评估），见 §Real-world test 的 poll_runs 计时。
- **页面本地刷新**：窗口显示时每 5 秒调用一次 `get_dashboard`，只读 SQLite；facts 查询最多扫描 500 行，有索引。窗口隐藏时不调用。
- **内存和 CPU**：私有内存合计约 260 MB，其中 WebView2 约 210–230 MB；隐藏时 CPU 约 0.03 s/分钟。详见 §Real-world test 的资源部分。

## Real-world test

**环境**

- Windows 11 Home China 26200，系统 UI 语言为中文；
- Codex Desktop（Store 版）0.162.0-alpha.2 同时在运行（其自身的两个 codex.exe：pid 20612、31052，作为基线）；
- ChatGPT Plus 账号；
- QuotaDock release exe 从 `target\release\` 运行（原因见 Known limitations 1）。

**过程**

验证中发现缺陷后会修复、重新构建，再用新的 exe 继续跑，共 5 段。前 4 段的数据仍是有效证据；最终二进制（第 5 段）完整重跑了一遍关键项目。

| 段 | 时间 | 二进制 | 结束原因 |
|---|---|---|---|
| 1 | 11:00–11:05 | 初版 | 发现关闭提示残留 → 修复 |
| 2 | 11:08–11:18 | + 提示修复 | 发现 provider error 误报，`--quit` 卡死 → 修复 |
| 3 | 11:25–11:31 | + 死锁修复 | 补 soak 日志（migration v3） |
| 4 | 11:31–12:05 | + provider_sessions | 发现隐藏时页面定时器仍在运行 → 修复 |
| 5（最终） | 12:05:51–14:06:00（120.3 分钟） | 最终版（sha256 前缀 `6F91A5F2021C`） | 跑满后 `--quit` |

**§70 验收项**

| 项目 | 结果 | 证据 |
|---|---|---|
| foreground 60s polling | ✅ | 第 5 段窗口显示期间，#80–#81、#87–#89 的间隔为 60–61 s；第 1–4 段同样是 60–61 s。窗口重新显示时，如果已经到期，会立即 poll 一次（#87 在 12:36:37 调出窗口时触发） |
| background 5min polling | ✅ | 隐藏后间隔为 300–302 s：第 4 段 #75–#79，第 5 段 #82–#86、#90–#106。第 5 段共 22 个 5 分钟间隔，范围 300–302 s，全部成功 |
| tray hide/show | ✅ | X → 关闭提示 → 确定 → 隐藏（`acknowledge_close_hint`）；同一会话第二次 X 直接隐藏；第二次启动会重新显示；最终段 14:04 通过托盘溢出区左键单击 QuotaDock 图标也重新显示了窗口。修复后，重新显示时不再残留关闭提示 |
| manual refresh | ✅ | `--refresh` 产生 manual poll（#28）；与上次 poll 间隔小于 10 s 时被节流（resume 后立即 refresh，没有产生 poll） |
| pause/resume | ✅ | 第 2 段：暂停后 app-server 立即退出，暂停的 2.6 分钟内 0 次 poll，resume 后 1 秒内 poll。最终段复测（14:04:36–14:05:05）：`--pause` 后 app-server 46840 退出，暂停期间 `--refresh` 没有把它拉起来；`--resume` 后 14:05:06 出现 manual poll（#107），新 app-server 61104 在 `provider_sessions` 中记为 `after_stop` |
| notification dedup | ✅（自动化测试）/ 真实运行未触发 | 额度充足（weekly 剩余 94–95%；5h 剩余 73–100%，且 5h 提醒默认关闭），没有跨越任何阈值，`notification_log` = 0、`notification_state` = 0，没有误报。按规格不人工消耗额度。toast 管道单独用 `--test-notification` 验证过 |
| SQLite writes | ✅ | 第 5 段 28 次 poll（启动 1、计划 26、手动 1）全部成功，每次一个 snapshot 加一条 poll_run；从 11:00 起全部 5 段共 89 次 poll，0 失败（含 quit 扫描时的启动 poll）；migration v3 在真实数据库上一次成功；数据库主文件 340 KB |
| no orphan codex process | ✅ | 最终段 14:06:00 `--quit`，2.2 s 内完全退出，只剩基线的 20612 和 31052，QuotaDock 的 WebView2 进程为 0。之前每次 `--quit` 也是如此；对卡死实例强杀（`Stop-Process -Force`）后，app-server 也被 Job Object 一并结束；quit 时序扫描中也没有残留 |
| single instance | ✅ | 第二次启动以 code 0 退出，进程数保持 1；argv 被转发给第一个实例 |
| language switch | ✅ | System（中文）→ English → 中文 → System，全程无需重启；页面、托盘 tooltip、test toast 同步切换。结束时托盘 tooltip 为 "QuotaDock / 5小时：73% · 每周：94% / 刚刚更新"；设置已恢复默认（language/theme = system） |

**其他实测**

- **toast**：`--test-notification` 发出的 "QuotaDock 测试通知 / 通知功能正常。" 和英文版 "QuotaDock test notification / Notifications are working." 都出现在该 AUMID 的通知中心历史里。查询只过滤含 "QuotaDock" 的条目。
- **深色主题**：在真实窗口中切到 Dark，页面和原生标题栏都变深；切回 System 后恢复浅色。
- **诊断导出**：真实导出的 `diagnostics-20261008-123826.json`（103 KB）扫描结果：邮箱、bearer、token、accountId、chatgptAccountId、installationId、`user-…`、`sk-…` 均为 0 命中。唯一的账号相关值是 1 个 32 位十六进制的 account fingerprint（不可逆，但在多次导出之间是同一个值）。
- **真实的 5h 空闲 → 活跃**：
  - 11:00–11:10 期间 5h 为 sliding，resetsAt 随每次读取前移，界面显示 "恢复使用后确认计时"，没有倒计时；
  - 11:10:58 resetsAt 固定在 16:10:08；
  - 11:14 起 5h 用量 0 → 16 → 18%、weekly 2 → 5%（你本人在 Codex 中的正常使用，验证脚本没有消耗额度）；
  - 界面变为倒计时 "Resets in 4h 37m"，没有产生误报事件；之后到 14:05，5h 用量继续升到 27%、weekly 升到 6%，都属于正常消耗，没有事件。
- **resetsAt 的 1 秒抖动**（真实后端行为）：
  - 从 5h 固定（#26）到运行结束（14:05），81 次相邻读取中，5h 出现 3 次 A→B→A，weekly 出现 1 次；
  - 12:53:40 两个窗口同时 +1 s，下一次又同时回来；
  - 每次都被判为 TransientBackendState（诊断，low），该点标为 `suspect_transient`，没有任何用户可见事件或通知；
  - `[INFERENCE]` 后端的 `reset_at` 可能是由当前时间加剩余秒数取整得到的。
- **一条真实诊断事件**：`reset_credit_count_changed`（2 → 3，increase，原因未知，low），发生在 QuotaDock 关闭期间；不显示在用户事件中，也不发通知。

**真实运行中发现的缺陷**

1. **退出死锁**（严重，已修复）
   - 现象：`--quit` 恰好落在一次 poll 进行中时，进程失去响应（"Not responding"），只能强杀。
   - 原因：
     1. `RunEvent::Exit` 在主线程上 `block_on(polling.stop())`，等正在进行的 poll 结束。
     2. 那次 poll 收尾时调用托盘 setter。
     3. Tauri 的托盘和菜单 setter 从后台线程调用时，会同步等待主线程执行 `[TAURI SOURCE]`，于是两边互相等待。
   - 修复：
     - `tray::apply` 改为 `run_on_main_thread` 投递，不再等待；
     - `quit()` 先在异步任务里停止 polling 和 app-server，再 `app.exit(0)`；
     - `RunEvent::Exit` 保留幂等的兜底清理。
   - 验证：同一时序扫描（启动后 300–3000 ms 发 `--quit`，各 10 次）：
     - 旧版 7/10 卡死（300–2100 ms，覆盖整个启动 poll）；
     - 新版 10/10 正常退出；
     - 另有一次在新编译 exe 首次冷启动时，300 ms 处出现一次未退出。复测 3 次加完整扫描 10 次都正常。`[INFERENCE]` 那次是 `--quit` 进程抢先拿到了单实例锁、自己退出，导致第一个实例没收到 quit，并不是死锁。
   - 所有卡死实例强杀后都没有遗留 codex.exe。
2. **关闭提示残留**（已修复）
   - 现象：提示显示时窗口被第二次 X 或托盘隐藏，再次打开时提示还在，点确定又会把窗口隐藏。
   - 修复：`hide_main` 调用 `hideCloseHint()`，只关闭提示、不确认。
3. **provider error 误报**（已修复，有回归测试）
   - 现象："15 分钟无有效数据" 是从最后一个 snapshot 起算的。QuotaDock 关闭几个小时后再启动，第一次读取只要失败一次，就会立即通知。
   - 修复：改为从本次失败连续段的第一次失败起算。测试 `provider_error_time_rule_counts_the_failure_streak_not_time_the_app_was_closed` 在旧逻辑下会失败。
4. **隐藏时页面定时器仍在运行**（已修复）
   - 现象：窗口隐藏到托盘后，WebView2 不知道自己不可见，1 秒时钟（每秒整页重渲染）和 5 秒本地读取一直在跑。
   - 实测：隐藏 25 分钟，整棵进程树 CPU +9.0 s（约 0.35 s/分钟）；渲染进程私有内存 61 → 142 MB。
   - 修复：后端在显示、隐藏、最小化时调用 `setActive(true/false)`，页面停止所有定时器。启动时如果窗口是隐藏的（autostart），用 dashboard 的 `polling.foreground` 判断。
   - 修复后：隐藏 3 分钟内整棵树 CPU 共 78 ms（约 0.026 s/分钟，降低约 13 倍）；渲染进程和 QuotaDock 进程都是 0 ms。最终段运行 31 分钟后，渲染进程私有内存 78 MB（旧版同等时长 142 MB）；运行 2 小时后为 54 MB，整棵树 240 MB。
5. **soak 日志缺口**（已补）：§72 要求记录 provider restarts，原先没有记录。新增 migration v3 `provider_sessions`（`initial` / `after_stop` / `after_exit` + pid + cli_version），并写入诊断包。有 provider 测试和 product 测试覆盖；真实数据库中已记录 `initial` 和 `after_stop`。

**发现但按 §73 暂不修改**

- **5h 分析段没有重新锚定**：
  - 现象：5h 从空闲转为活跃后，`analysis_segment_start_snapshot_id` 仍是 02:23:33 的 #13（自然滚动之后的空闲段起点），而真实窗口从约 11:10:08 开始（16:10:08 − 5 h）。
  - 后果：5h burn rate 显示 1.8–1.9 pp/h，forecast 显示 Low，实际是几分钟内用掉 18 pp。History 页面的 "近期消耗（6 小时）" 为 6.3 pp/h，更接近真实。
  - 证据：redacted fixture `tests/fixtures/real-5h-idle-to-active-2026-10-08.json`（58 个 snapshot，只有规范化数值）。
  - 修复需要改 detector 并 bump classifier 版本。GPT 的指示是 "Phase 3 不再频繁修改 Core Engine"，所以留到 soak 期间按 §73 处理。

**资源**（第 5 段，私有内存）

- QuotaDock 进程 13–14 MB；app-server 27–28 MB；WebView2 6 个进程约 210–230 MB；合计约 260 MB。
- CPU：窗口显示时约 0.6 s/分钟（含 1 秒时钟和 60 s poll）；隐藏时约 0.03 s/分钟，加上每 5 分钟一次 poll。
- 单次 poll 0.39–1.53 s。运行结束时数据库主文件 340 KB。

## Known limitations

1. **Windows toast 身份（未打包）** `[TAURI SOURCE: tauri-plugin-notification-2.5.1/src/desktop.rs:222-229]`
   - 插件只在 exe 位于 `target\debug` 或 `target\release` 目录时，才使用系统已注册的 PowerShell AUMID；在其他目录会用 `com.quotadock.app`。这个 AUMID 没有注册，Windows 会**静默丢弃** toast。
   - 因此真实运行使用 `target\release\quotadock-app.exe`，toast 来源显示为 Windows PowerShell。
   - 正式身份需要安装器注册 AUMID（Phase 4）。
2. **`delivered = 1` 只表示已提交**：插件在后台任务里调用 `show()` 并丢弃结果，QuotaDock 拿不到 "已显示" 的确认。真实运行中通过查询该 AUMID 的通知中心历史，确认 toast 确实到达。
3. **5h 分析段在 "空闲 → 活跃" 后没有重新锚定**（真实发现，Phase 3 按 §73 只保留证据，未修改）。详见 §Real-world test，证据在 `tests/fixtures/real-5h-idle-to-active-2026-10-08.json`。
   - 影响：空闲很久后首次使用，5h burn rate 和 5h forecast 会明显偏低。本例显示 1.8 pp/h，实际是几分钟内用掉 18 pp。
   - 不影响托盘 severity（不使用 5h forecast），也不影响默认通知（5h forecast 通知默认关闭）。
   - weekly 在空闲后是否有同样的问题 `[UNKNOWN]`，取决于 weekly 的空闲语义，需要 soak 观察。
4. **resetsAt 的 1 秒抖动**：真实运行中出现了 16:10:08 → 16:10:09 → 16:10:08，被正确判为 TransientBackendState（诊断，low），该点被排除出分析。频率需要 soak 统计；如果很频繁，可以考虑 ±1 s 容差，但必须走 §73 流程。
5. **WebView2 内存**：窗口隐藏后 webview 仍常驻，约 210 MB 私有内存（6 个进程），占整棵进程树的绝大部分。QuotaDock 自身约 13 MB，app-server 约 30 MB。
6. **单实例激活的前台锁**：脚本触发的第二次启动会显示窗口，但 Windows 可能不允许把它切到前台。
7. **开机自启**没有在真实运行中打开。它会写 HKCU Run 键，属于持久配置，本次验证没有替用户修改；功能来自官方插件，默认关闭。
8. **通知去重的真实触发**：真实运行期间额度充足（weekly 剩余 94–95%；5h 剩余 73–100%，而 5h 提醒默认关闭），没有跨越任何阈值。按规格要求不人工消耗额度，所以去重只由自动化测试验证，真实运行中 `notification_log` 为 0 条，没有出现误报。
9. **daemon proxy 仍未启用**，只使用 stdio app-server（同 Phase 2）。
10. **无障碍**：语义按钮、`aria-*`、键盘焦点、形状加颜色的图标都已实现，UI Automation 也能完整遍历页面（本次自动化就是这样做的）；但没有做读屏软件的人工审查。
11. **一次来源不明的 manual poll**：第二轮运行中 11:08:57 出现一次 manual poll，不是验证脚本触发的。只有 Refresh 按钮、托盘 "立即刷新"、`--refresh` 和 Resume 会产生 manual，推测是有人点了刚弹出的窗口里的刷新按钮。
12. **诊断包中的 account fingerprint**：诊断包包含完整的 32 位十六进制 fingerprint。它是不可逆的 SHA-256 截断，但在多次导出之间是同一个值。公开分享诊断包前，可以考虑改为只导出 8 位短 fingerprint（Phase 4 小项）。

## 7-day soak plan

目标（§71）：连续运行 ≥ 7 天，重点观察 weekly rollover、weekly 空闲行为、resetsAt transient 频率、provider 重启、OAuth 问题和 Store 版 Codex 的更新行为。

**运行方式**

- 运行 `target\release\quotadock-app.exe`（这个目录保证 toast 可见）。
  - 是否打开 "开机启动" 由你决定。打开后重启电脑也能继续；不打开的话，重启后需要手动启动。
- 保持默认设置；不要人工消耗额度来制造事件。
- 每天运行一次只读报告：

```bash
node scripts/soak-report.mjs --days 1
```

  结束时再运行 `--days 7`。

**报告的内容覆盖 §72 的全部项目**

| §72 项目 | 来源 |
|---|---|
| successful / failed polls | `poll_runs`，按 status 和 trigger 统计，另列最长间隔和最近的失败 |
| provider restarts | `provider_sessions`（新 migration v3），`after_exit` 表示子进程自己退出，`cli_version` 变化表示 Codex 更新 |
| NaturalRolloverObserved / SlidingAnchorDetected / TransientBackendState / WindowScheduleChanged / UnscheduledQuotaRefresh | `quota_events`，按类型、窗口、可见性、置信度统计，并单列 weekly rollover |
| TemporarilyMissing | 各窗口的 `missing_window` snapshot 数 |
| resetsAt 抖动频率 | 相邻 snapshot 之间 1–2 s 的 resetsAt 变化次数 |
| 通知 | `notification_log`，按规则和语言统计 |

**需要人工确认的事项**

- **weekly rollover**（本账号下一次在 10月14日 21:19）：
  - 应出现 `natural_rollover_observed/weekly`；
  - Overview 应显示 "每周额度已刷新"；
  - 不应出现 "新窗口开始" 之类的措辞。
- **Store 版 Codex 更新**：
  - `provider_sessions` 里应出现新的 `cli_version`；
  - 版本门禁的行为：比已验证版本（0.162.0-alpha.2）旧的版本会被拒绝；更新的版本按 `unverified-newer` 以兼容模式接受，dashboard 会标出兼容模式。
- **Windows 睡眠和唤醒**：唤醒后应立即 poll；托盘的 30 分钟过期 error 应在唤醒后的第一次成功 poll 时消失。
- **在官方 app 中登出再登录 Codex**：
  - 托盘应变为 error（未登录）；
  - 满足 3 次失败或 15 分钟条件后，应发一条通知；
  - 重新登录后应自动恢复，且不发 "连接已恢复" 通知。

**如果发现新的后端行为（§73）**

1. 保留原始证据：导出诊断包，或像本次一样导出 fixture。
2. 写回归测试。
3. 修改 detector。
4. 提升 classifier 版本。
5. 用 `quotadock-dev reanalyze` 重放历史。

## Phase 4 recommendation

按优先级排列：

1. **先完成 §71 的 7 天 soak**，同时按 §73 修复 5h 分析段的重新锚定问题：
   - 证据已有，下一步写回归测试，然后修改、bump 成 `reset-detector-v2`、reanalyze；
   - 这是目前唯一已知会让 UI 显示失真的问题。
2. **打包和身份**：
   - MSIX 或 NSIS 安装器，注册 AUMID 和开始菜单快捷方式，让 toast 以 QuotaDock 身份显示（解决 Known limitations 1）；
   - 签名；
   - 安装器仍然不包含自动更新（V1 规则）。
3. **降低隐藏时的内存**：窗口隐藏一段时间后销毁 webview、显示时重建（状态都在 Rust 和 SQLite 里，重建没有代价），或者使用 WebView2 的 `TrySuspend`。预期能省下大部分约 210 MB 的私有内存。
4. **Claude provider**：
   - 在 soak 通过后开始；
   - 复用 provider 中立的 DTO（`ProviderDashboard`、`WindowStatus`、`HealthState`）和通知、托盘规则；
   - 前端组件已经按 provider 列表渲染。
5. **小项**：
   - 单实例激活时用 `AllowSetForegroundWindow` 改善前台切换；
   - History 图表加入可访问的数据表视图；
   - 设置页显示 "上次通知" 审计列表（数据已经在 `notification_log` 中）。

## 附录 A：如何运行

```bash
cd ui && npm install && npm run build
```

```bash
cargo build --release -p quotadock-app
```

- 运行 `target\release\quotadock-app.exe`。放在这个目录下，未安装时 toast 也能显示。
- 命令行参数会转发给正在运行的实例：
  - `--quit`、`--pause`、`--resume`、`--refresh`、`--test-notification`：只执行对应动作，不弹出窗口；
  - `--hidden`：启动时不显示窗口；
  - `--autostart`：由开机自启使用。
- 数据目录：`%LOCALAPPDATA%\QuotaDock\`，含 `quotadock.sqlite3`、`quotadock.lock`、`exports\`。
- 只读的 soak 报告：

```bash
npm run soak:report -- --days 7
```

- 浏览器预览（mock 数据，不连接后端）：在 `ui/` 下运行 `npm run build`，再运行 `npm run preview`，然后打开 `http://localhost:4173/?scenario=low&lang=zh-CN&theme=dark`。
  - scenario 可选 normal、sliding、low、blocked、unknown、error、notfound、unauth、connecting、paused。

## 附录 B：测试清单

**Rust：81 个**

- analytics 7、engine_scenarios 25、parity 2：Phase 2 原有。
- polling 6：新增 `pause_stops_scheduling_and_resume_polls_immediately`。
- rpc_and_provider 9：新增 `provider_reports_each_app_server_start_with_its_reason`。
- storage 11：schema 升到 v3。
- product 21：
  - availability：3 个（false→true 只通知一次；null→true 和启动时 true 不通知；长间隔不算转换）；
  - 阈值：3 个（weekly 去重跨重启、新周期重新武装；启动时 ≤10% 只通知一次；5h 默认关闭）；
  - 窗口缺失 1 个；
  - provider error：2 个（3 次失败、每个连续段一次；时间规则按失败连续段计算）；
  - forecast 每周期一次 1 个；
  - 合并和优先级聚合：2 个；
  - 诊断事件不通知、credit 需要 HIGH：1 个；
  - 托盘：4 个（优先级、未知持续、5h 开关、暂停和过期）；
  - 暂停：1 个（停止 app-server、不持久化、记录 provider 启动）；
  - settings 1 个；history 2 个。

**UI：20 个**（`node --test`）

- `present.test.ts`：reset 文案（sliding 不显示倒计时，>24h 显示日期）、availability、新鲜度、forecast 模式、事件标题等。
- `chart.test.ts`：分段和缺口断线、坐标、时间刻度。

**TS 参考实现：83 个**（Phase 1B），仍全部通过。

**静态检查**：`cargo clippy --workspace --all-targets` 零警告；`cargo fmt`；`tsc --noEmit`（TypeScript 7）。

## 附录 C：与规格的差异和补充

| 项目 | 说明 |
|---|---|
| provider error 的 "15 分钟" | 解释为 "失败连续段已持续 15 分钟"，而不是 "距最后一个 snapshot 15 分钟"。后者会在长时间关闭后误报，见 §Real-world test 缺陷 3 |
| availability 恢复的 30 分钟间隔 | `[QUOTADOCK HEURISTIC]`，用来实现 "startup true 不通知"：QuotaDock 关闭期间跨越的 false → true 不通知 |
| 关闭提示 | "本次会话第一次" 关闭时显示（规格是 one-time hint）。勾选 "不再提示" 后永久不再显示 |
| migration v3 `provider_sessions` | 规格外新增，为了满足 §72 的 provider restarts 记录 |
| `notification_log.delivered` | 含义是 "已提交给 Windows"，插件不返回显示结果 |
| 5h 分析段重新锚定 | 真实发现，但按 §73 和 "Phase 3 不频繁改 Core Engine" 的指示，只保留证据，没有修改 |
| 页面定时器 | 规格没有要求；为了托盘常驻时的资源占用，隐藏时停止 |
| forecast margin 符号 | 修正为与规格示例一致（正值表示有余量）；Phase 2 文档已同步说明 |

## 附录 D：主要文件

- **core**：
  - `crates/quotadock-core/src/product/`：`facts`、`tray`、`notifications`、`history`、`dashboard`、`runtime`、`settings`、`i18n`；
  - `polling/controller.rs`（Pause/Resume）；
  - `storage/migrations.rs`（v2、v3）；
  - `codex/provider.rs`（`ProviderStart`）。
- **shell**：`src-tauri/src/`（`main.rs`、`commands.rs`、`tray.rs`、`notify.rs`、`window.rs`）、`src-tauri/capabilities/default.json`、`src-tauri/build.rs`、`src-tauri/icons/tray/*.png`。
- **UI**：`ui/src/`（`api/`、`i18n/`、`model/`、`components/`、`pages/`、`App.tsx`、`styles.css`）。
- **测试和证据**：
  - `crates/quotadock-core/tests/product.rs`；
  - `tests/fixtures/real-5h-idle-to-active-2026-10-08.json`；
  - `scripts/soak-report.mjs`。

## 附录 E：真实窗口截图

只截 QuotaDock 自己的窗口，位于 `docs/phase3/`：

- `01-overview-zh-idle-sliding.png`：中文空闲态，5h 显示就绪和 "恢复使用后确认计时"
- `02-close-to-tray-hint-zh.png`：关闭到托盘的提示
- `03-paused-en.png`：英文暂停态
- `04-overview-en-active-countdown.png`：首次使用后 5h 出现倒计时，以及 forecast 数值
- `05-overview-zh-dark.png`：深色主题加中文
- `06-history-zh.png`：24h 历史图，可以看到滚动断线、标记线和关闭期间的缺口
- `07-events-zh.png`：用户事件 "5 小时额度已刷新"
- `08-settings-en.png`：设置页（英文）
