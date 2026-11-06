# QuotaDock Phase 3.8 Core Hotfix Findings

Windows V1 · Final Integration Stage A（Core Hotfix）· branch `v1-soak-baseline` · report date 2026-10-26 · Windows 11 Home China 10.0.26200

依据标记沿用前几阶段，另加一个：
- `[REAL RUN]`：真实运行。
- `[REPLAY]`：在生产数据库的**只读拷贝**上完整重放全部真实快照（新增）。
- `[LOCAL TEST]`：自动化测试。
- `[QUOTADOCK HEURISTIC]`：QuotaDock 自定的规则。

**如何得到这些数字：** 测试结果来自本机运行；历史对比来自生产数据库的两份拷贝（通过 SQLite 在线备份经只读连接得到），一份保持 v2 原样，一份用 v3 完整重分析。没有启动、停止或替换生产程序，也没有写入生产数据库。

## 结论

**Stage A 完成，§10 的合并前置条件全部满足，可以进入 Stage B（同步到 Widget 分支、建立 RC）。** 切换生产前又做了一次独立代码审查（§6）：复现成立的 6 个问题中，5 个已修复（`4204f0e`、`c38a9d0`），1 个不影响本次升级，写入 backlog。

| §10 前置条件 | 结果 | 依据 |
|---|---|---|
| Detector-v3 regression PASS | ✅ | §6 矩阵 18 项全部覆盖；新测试在旧逻辑下 9 / 20 失败，在新逻辑下 20 / 20 通过（§3）；独立审查后再加 4 项，修复前失败、修复后通过（§6） |
| Historical reanalysis PASS | ✅ | 10,609 个真实快照完整重放；卡住消失，其余分类不变，两次重放结果逐字节相同（§4） |
| Claude logout notification PASS | ✅ `[LOCAL TEST]` | 状态机 6 项单元测试 + 2 项集成测试（fake Claude CLI）；真实环境 **NOT VERIFIED**（§5） |
| Full Core tests PASS | ✅ | `cargo test --workspace` 181 项、clippy（`-D warnings`）、UI 类型检查和 24 项测试（§7）；独立审查后的修复提交 188 项 |
| No unresolved P0 | ✅ | 已知 P0（每周窗口卡住）已修复；真实运行验证留给 Stage D |
| Database migration safe | ✅ | 无 schema 变更；v3 行与 v2 行并存，旧行和通知记录保留；回滚方法见 §4.4 |

仍未在真实运行中验证（Stage C / D 处理，不在本报告中写 PASS）：
- v3 在真实轮询中的行为（需要切换生产版本，须用户同意）。
- Claude 退出登录通知作为真实 Windows toast 的显示（规格 §24：不为验证而退出用户的真实账号）。

---

## 1. 提交与范围

| 提交 | 内容 |
|---|---|
| `6be0d1c` | reset-detector-v3：修复每周（及 5 小时）窗口提前刷新后卡住；`CLASSIFIER_VERSION = reset-detector-v3`；提前刷新通知文案；真实回归 fixture；20 项回归测试和 1 项产品层测试 |
| `fb8c32a` | Claude 退出登录通知：持久化的 episode 状态机（`product/claude_auth.rs`），接入 `claude_tick` |
| 本报告所在提交 | 本报告、`POST_V1_BACKLOG.md` 第 26–32 项、`SOAK_BASELINE_MANIFEST.md` 记录 |
| `4204f0e` | 独立审查后修复（§6）：reset 时间附近的空闲读数是 rollover；"被取代" 安全阀不再吞掉刷新 / rollover |
| `c38a9d0` | 独立审查后修复（§6）：退出登录通知在写入成功后才算已通知；检查期间暂停 / 停用不发；合并通知时总是标题 |

**范围控制（§9）：** 没有改动 PollingController、Forecast engine、provider adapter、WebView2 生命周期，也没有改动 reset detector 之外的 analytics。报告中发现的其他问题都写入 backlog（§8）。

## 2. P0：reset-detector-v3

### 2.1 根因（回顾）

详见 `WINDOWS_V1_CORE_SOAK_FINDINGS.md` §3。窗口从固定重置时间变成空闲（用量 0%，重置时间 = 读取时间 + 窗口时长，随时钟移动）时：
- 下降小于 10 个百分点，不算刷新；旧重置时间未到，也不算自然重置；于是开 `Unknown` 候选。
- `Unknown` 要求下一次读数的重置时间完全相同，而空闲窗口的重置时间每次都变。
- 结果是每次轮询都拒绝上一个候选，再用同一个旧参照点开新候选，参照点永远不前进。

**对 Core soak 报告的更正：** 该报告 §3 写 "5 小时窗口没有这个问题"。重放发现这不完整。10-18 05:41:01，两个窗口**同时**提前刷新：
- 每周：43% → 0%，超过 10 个百分点，v2 正确确认。
- 5 小时：6% → 0%，离原定重置（08:07:16）还有 2 小时 26 分钟。v2 对它卡住了 2.5 小时：
  - 146 条诊断事件；
  - 直到旧重置时间过去，才在 08:08:15 记成一次 "自然重置"（时间晚了约 2.5 小时）。

v3 把它记为 05:41 的 5 小时提前刷新（§4.2）。因此 Core soak 报告里第 2 轮 "17 次 5 小时自然重置" 应为 16 次自然重置加 1 次 5 小时提前刷新。

### 2.2 修复

**新的候选类型 `BecameIdle`。** 开启条件，全部满足：
- 新读数是空闲 sliding anchor；
- 参照点不是空闲的；
- 窗口时长相同；
- 还没到参照点的重置时间；
- 下降不足 10 个百分点（大幅下降仍走原来的 `QuotaRefresh` 路径）。

**连续性（§5.3）：**

| §5.3 要求 | v3 实现 |
|---|---|
| `resetsAt - capturedAt ≈ windowDuration` | 每个读数都必须是 sliding-like（偏差 ≤ 120 s，沿用 v2 的 `sliding_anchor_tolerance_secs`） |
| usedPercent 持续接近 0 | sliding-like 要求用量为 0 |
| resetAt 随 capturedAt 推移，drift 与时间一致 | 与上一个点比较：\|Δreset − Δcaptured\| ≤ 30 s（新 `idle_anchor_drift_tolerance_secs`） |
| window duration 一致 | 与开启点的时长相同 |
| 数据质量有效 | 短暂缺失（少于 3 次轮询）不计入也不打断，持续缺失会拒绝候选；用量为空等畸形读数会中断候选，不会确认 |
| account context 一致 | 账号变化会关闭整个比较上下文（v2 起即如此），候选被拒绝 |
| 多次连续有效观测才确认 | 连续 3 个空闲读数（新 `idle_confirm_polls`，含开启点）；单次 3% → 0% 不确认 |

**确认后的语义（§5.2）：**
- 用量下降超过噪声（> 1 个百分点）：`UnscheduledQuotaRefresh`，置信度 `Medium`，`cause: "unknown"`。
- 在噪声范围内（例如 1% → 0%、0% 固定 → 0% 空闲）：只记为 `WindowScheduleChanged`（诊断），不对用户显示。
- 证据中标出 `toIdleSlidingAnchor`、空闲读数个数和全部点。
- 不推断任何机制：
  - 不推断促销重置、reset credit 或服务器确认的原因；
  - 不推断新的计费窗口：新周期的开始只是 "首次观察到空闲的时间"，空闲窗口没有推导的窗口开始时间。

**边界情况：**
- **候选确认前旧重置时间已到：** 转为自然重置（`Rollover`）。这样，在重置边界因时钟偏差提前几秒看到的空闲，不会被说成提前刷新（测试见 §3 第 11 项）。
- **reset credit：** 下降超过噪声，并且 2 次轮询内 credit 减少，才记为 `ResetCreditRedeemed`。这是 v2 对大幅刷新已有的强证据规则。
  - 10-23 那次，credit 3 → 2 发生在 18 次轮询之后，不相关，不会被归因为 reset credit。
- **大幅下降进入空闲（`QuotaRefresh`）：** 置信度规则补了一条：连续一致的空闲读数也算稳定证据。v2 因空闲窗口的重置时间从不重复，总给 `Low`。历史中 4 次每周提前刷新因此从 `Low` 变为 `Medium`，事件键不变。
- **backend reconciliation：** 确认后 3 次轮询内回到旧状态，会照旧撤销（新周期作废、旧周期重开），同时撤销 sliding 声明。

### 2.3 恢复（§5.4）

| §5.4 要求 | v3 |
|---|---|
| 旧的 confirmed resetAt 不再显示为有效 | 候选待定期间，只要最新待定读数是空闲的，就不再把旧重置时间作为 "已确认" 呈现。确认后窗口是 sliding anchor，没有确认的重置时间 |
| Sliding idle state 正确建立 | 确认时直接设为 sliding（证据随事件记录，不另写 `SlidingAnchorDetected`） |
| Accepted reference 正常前进 | 确认时接受全部候选点，之后的空闲读数按 sliding 接受 |
| Pending candidate 能确认或安全失效 | 确认；或被 A→B→A / 账号变化 / 畸形读数拒绝；或在旧重置时间到达时转为自然重置。另有安全阀：同一参照点上连续 12 个候选都被新值取代时，以最新读数为参照点（1 条诊断） |
| Weekly history / threshold evaluation 恢复 | 空闲读数恢复为正常质量（重放：每周 957 个被排除的快照恢复） |
| 不持续生成重复 diagnostics | 真实 fixture：v2 119 条事件 → v3 4 条；一串 "被取代" 的拒绝只写第一条诊断 |
| 未确认时不无条件标为 confirmed | 同第 1 行：待定期间呈现为 "无确认重置时间" |

### 2.4 Cycle 语义（§5.5）

- 一次转变只产生一个 cycle 边界，在确认时创建，开始于首次观察到空闲的快照。
- 之后空闲窗口每次读数的重置时间变化都被当作 sliding，不会开新周期，也不会重复写刷新事件。
- 推导的窗口开始时间（`resetsAt − duration`）仍只用于 v2 的 idle → active re-anchor，从不作为服务器时间。
- 5 小时 idle → active re-anchor 保持不变：v2 的 6 项测试（含 10-08 真实 fixture）全部通过。每周窗口在提前刷新后恢复使用时，也走同一个 re-anchor（新测试）。

### 2.5 容差 `[QUOTADOCK HEURISTIC]`

| 参数 | 值 | 依据 |
|---|---|---|
| `percent_noise_tolerance_pp` | 1（未变） | usedPercent 是四舍五入的整数 |
| `sliding_anchor_tolerance_secs` | 120（未变） | 判断 "重置时间 = 现在 + 时长" 的绝对偏差；真实数据 0–1 s |
| `idle_anchor_drift_tolerance_secs` | 30（新） | 相邻两次空闲读数之间的相对漂移；真实 10-23 数据最大 2 s；30 s 对应读取超时上限 |
| `idle_confirm_polls` | 3（新） | 与 `sliding_min_samples` 一致；后台节奏下约 10 分钟 |
| `max_superseded_candidates` | 12（新） | 只是安全阀；后台节奏下约 1 小时；10,609 个真实快照的重放中没有触发 |

没有对所有窗口统一使用一个宽松容差：绝对偏差和相对漂移分开判断，漂移的容差比偏差严格 4 倍。

### 2.6 文案

| | 中文 | English |
|---|---|---|
| 标题 | Codex 额度提前刷新（每周） | Codex quota refreshed early (Weekly) |
| 正文 | 观察到 Codex 每周额度提前刷新，具体原因未知。 | QuotaDock observed an early refresh of the Codex weekly allowance. The cause is unknown. |

5 小时窗口的正文把 "每周" 换成 "5 小时"。

## 3. 回归测试矩阵（§6）`[LOCAL TEST]`

**旧逻辑的证明方式：** 把实现文件临时恢复到 `73bdcb4`（v2），用同一份测试文件运行，记录结果后再恢复 v3。

**真实 fixture：** `tests/fixtures/real-weekly-idle-refresh-2026-10-23.json`。
- 内容：快照 #9615–#9770，10-22 23:00 – 10-23 12:00，共 156 个，id 连续。
- 已脱敏：只有时间、百分比、重置时间、可用性、credit 数和 bucket id，没有账号标识。

| # | §6 项目 | 测试（`detector_v3.rs`，除非另注） | v2 | v3 |
|---|---|---|---|---|
| 1 | Weekly 3% fixed → 0% sliding → 持续 sliding | `real_weekly_idle_refresh_is_confirmed_once` | ❌ | ✅ |
| 2 | Sliding anchor 连续确认 | 同上（第 3 个空闲读数 #9655 确认）；`idle_needs_consecutive_observations_and_a_return_is_transient` | ❌ / ✅ | ✅ |
| 3 | Accepted reference 正常推进 | 同 1（参照点 = #9770）；`small_drop_within_noise_into_idle_is_not_a_refresh`；`a_drifting_backend_value_does_not_freeze_the_reference` | ❌ | ✅ |
| 4 | 旧 confirmed reset time 清除 | 同 1（#9653 时不再呈现 10-29 00:21，最终无确认时间） | ❌ | ✅ |
| 5 | History 不再持续断档 | 同 1（#9653 起全部为正常质量） | ❌ | ✅ |
| 6 | 诊断事件不无限增长 | `real_replay_writes_four_events_in_total`（v2 为 119 条）；drifting 测试（40 次轮询 ≤ 8 条） | ❌ | ✅ |
| 7 | Refresh event 恰好一次 | 同 1；`missing_or_malformed_weekly_does_not_create_a_false_refresh` | ❌ | ✅ |
| 8 | A→B→A transient 不误报 | `idle_needs_consecutive_observations_and_a_return_is_transient`；fixture 中 #9616 的 1 秒抖动 | ✅ | ✅ |
| 9 | 仅 resetAt 微小变化不误报 | `usage_noise_and_reset_jitter_are_not_refreshes` | ✅ | ✅ |
| 10 | 1pp 用量噪声不误报 | 同上；`small_drop_within_noise_into_idle_is_not_a_refresh`（v2 在此卡住） | ✅ / ❌ | ✅ |
| 11 | 正常 5h rollover 不受影响 | `natural_five_hour_rollover_is_unaffected`（4% / 35%）；`a_window_idle_seconds_before_its_reset_is_a_natural_rollover`；`five_hour_window_is_unaffected_on_the_real_sequence` | ✅ | ✅ |
| 12 | 正常 weekly rollover 不受影响 | `natural_weekly_rollover_is_unaffected` | ✅ | ✅ |
| 13 | 5h idle→active re-anchor 不受影响 | `five_hour_idle_to_active_reanchor_is_unaffected`；`detector_v2.rs` 6 项；`usage_after_an_idle_refresh_reanchors_the_analysis_segment`（v2 ❌） | ✅ | ✅ |
| 14 | Candidate 跨重启恢复 | `restart_mid_candidate_keeps_the_outcome`（在待定候选处持久化再继续，结果与连续回放一致） | ✅ | ✅ |
| 15 | Account change 隔离 | `account_change_during_an_idle_candidate_is_isolated` | ✅ | ✅ |
| 16 | Missing / malformed snapshots 不误报 | `missing_or_malformed_weekly_does_not_create_a_false_refresh` | ❌ | ✅ |
| 17 | Replay deterministic | `replay_is_deterministic`；真实历史两次完整重放逐字节相同（§4.2） | ✅ | ✅ |
| 18 | Notification dedup 不受影响 | `notification_scopes_ignore_the_classifier_version`；`product.rs::early_refresh_notifies_once_live_and_a_classifier_replay_never_resends_it`（v2 ❌） | ✅ | ✅ |

另外 3 项（v2 均 ❌）：
- `substantial_drop_into_idle_is_a_medium_confidence_refresh`（v2 为 `Low`）；
- `idle_refresh_with_a_correlated_credit_decrease_is_a_redemption`；
- `a_quick_return_to_the_old_anchor_after_an_idle_refresh_is_reconciled`。

**合计：** `detector_v3.rs` 在 v2 下 11 通过 / 9 失败，在 v3 下 20 / 20 通过。v2 下通过的 11 项正是 "不应改变" 的行为（噪声、抖动、A→B→A、自然重置、re-anchor、账号隔离、重启、确定性）。

## 4. Classifier v3 与历史重分析（§7）

### 4.1 方法 `[REPLAY]`

1. **生产程序：** 10-26 09:16 Windows 重启后没有再运行（没有开机自启），本阶段没有启动它。
2. **拷贝：** 用只读连接做 SQLite 在线备份，得到 `prod-copy-20261026.sqlite3`。
   - 两边 `quick_check: ok`；快照都是 10,609 个（#1–#10609，10-08 01:23 – 10-26 09:15）。
   - 生产主文件和 WAL 的大小、修改时间在拷贝前后不变。
   - 只有 `-shm`（WAL 的共享内存索引）的修改时间变了；任何读连接都会更新它，它不含持久数据。
3. **v2 基线：** 从拷贝再复制一份保持原样，作为 v2 基线。
4. **v3 重分析：** 另一份用 v3 的 `quotadock-dev`（release，`target/alt`）打开。
   - 打开即触发与生产首次启动完全相同的自动重分析；
   - 之后再显式执行一次 `reanalyze`。
   - 只用离线命令；`status` 会真实轮询 Codex，没有使用。
5. **耗时：** 10,609 个快照的完整重分析用时 **2.4 s**（release）。这也是生产首次启动 v3 时额外的启动时间。

所有拷贝和工具都在 `target/qd-tools`（gitignored），生产数据从未被写入。

### 4.2 结果

**事件（全部历史）：** 1,522 → 420。

| 类型 · 窗口 | v2 | v3 |
|---|---|---|
| `transient_backend_state` weekly | 1,106 | 150 |
| `transient_backend_state` 5h | 260 | 114 |
| `unscheduled_quota_refresh` weekly（user） | 5 | 6 |
| `unscheduled_quota_refresh` 5h（user） | 0 | 1 |
| `natural_rollover_observed` 5h（user） | 26 | 25 |
| `sliding_anchor_detected` 5h | 23 | 22 |
| 其他类型（reconciliation、schedule change、credit 变化、weekly sliding） | 不变 | 不变 |

**用户可见事件的变化（其余 26 条完全相同）：**

| 时间 | v2 | v3 |
|---|---|---|
| 10-11 13:00 weekly 10% → 0% | 提前刷新，`Low` | 提前刷新，`Medium`（同一事件键） |
| 10-13 18:13 weekly 27% → 0% | 提前刷新，`Low` | 提前刷新，`Medium` |
| 10-18 05:41 weekly 43% → 0% | 提前刷新，`Low` | 提前刷新，`Medium` |
| 10-18 05:41 5h 6% → 0% | 卡住，08:08 记为自然重置（`Low`） | **5 小时提前刷新，`Medium`**（§2.1 更正） |
| 10-20 15:33 weekly 10% → 0% | 提前刷新，`Low` | 提前刷新，`Medium` |
| 10-23 02:12 weekly 3% → 0% | **没有记录**（卡住） | **每周提前刷新，`Medium`**，#9652 → #9655 |

**周期与边界：**
- **5 小时：** 27 → 27 个。10-18 的边界从 08:08（`natural_rollover_observed`）移到 05:41（`unscheduled_quota_refresh`）。
- **每周：** 6 → 7 个。新周期从 #9653（10-23 02:12:58）开始。v2 中从 #6661 开始的周期一直未关闭；v3 中在 #9652 关闭。
- 两边都没有作废的周期。

**最终窗口状态（最后快照 #10609）：**

| | v2 | v3 |
|---|---|---|
| 每周 "已确认" 重置时间 | **10-29 00:21:41（过期）** | 无（空闲，sliding） |
| 每周参照点 / 待定候选 | #9652 / `Unknown`（自 #10609） | #10609 / 无 |
| 每周分析段起点 | #8611 | #9653 |
| 5 小时 | 空闲，sliding，周期 #8907 | 相同 |

**候选：**
- `unknown_transition` 的拒绝：每周 956 → 0，5 小时 146 → 0；
- `became_idle` 确认：每周 0 → 1，5 小时 0 → 1；
- 5 小时 `rollover` 确认 26 → 25（10-18 那次改记为提前刷新）；
- 其他类型和状态的数量完全相同，例如 `schedule_change`（每周 / 5 小时）被拒绝 150 / 113、确认 16 / 37。

**分析段：**
- 5 小时：段数 48 → 48；10-18 的段起点从 #3479 变为 #3333。
- 每周：段数 10 → 11；新增 #9653。

**快照质量：**

| | v2 | v3 |
|---|---|---|
| 每周 normal / suspect / pending | 9,470 / 1,138 / 1 | 10,427 / 182 / 0 |
| 5 小时 normal / suspect | 10,269 / 340 | 10,415 / 194 |
| 每周 ≥ 3 个的连续非正常段 | 3 段，其中 #9653–#10609（957 个，10-23 02:12 – 10-26 09:15） | 2 段（各 3 个，10-09 和 10-16） |

**逐快照状态：**
- 两个窗口的观察状态分布（active / idle / schedule change candidate / stable）完全相同，状态切换次数也相同（344 / 343）。
- 待定候选的快照：每周 1,133 → 178，5 小时 323 → 178。
- 呈现 "已确认重置时间" 的快照：每周 8,416 → 7,451，5 小时 4,453 → 4,283。减少的部分是：
  - v2 呈现过期重置时间的时段（每周 10-23 起，5 小时 10-18）；
  - 待定读数已是空闲的轮询（例如自然重置前那一拍，旧重置时间已经过去）。

**确定性：** 同一份拷贝连续两次完整重分析，导出的 v3 数据逐字节相同（sha256 `43ce7eaa…`）。

### 4.3 通知影响

- **已发送的去重记录：** 共 18 条。按周期和事件限定的 7 条（5 条刷新、2 条预测）去掉版本号后，在 v3 中都能找到对应的事件或周期，升级后不会重发。另外 8 条 provider error、1 条可用性恢复、2 条 Claude 提醒与 classifier 无关。
- **重放事件：** 10-23 每周刷新、10-18 5 小时刷新这些重放出来的事件不会被当作实时事件补发。
  - 结构上，事件通知只来自一次实时轮询的结果；重分析不经过通知评估。
  - 测试 `early_refresh_notifies_once_live_and_a_classifier_replay_never_resends_it` 删除了去重记录后再重分析，仍然不发。
- **首次 v3 轮询：** 最后快照剩余 5 小时 100%、每周 100%，不会触发额度提醒。新的每周周期 #9653 还没有提醒记录，这是正确的。

### 4.4 迁移与回滚

**数据库：**
- 没有 schema 变更（仍是 v6），没有新的迁移。
- v3 只新增自己的派生行：420 条事件、34 个周期、349 个候选、2 条窗口状态、1 条引擎状态。
- v2 的派生行、v1 的派生行、原始快照和通知记录都保留。
- 与 v1 → v2 时一样，每个快照的共享质量列会按 v3 重写。
- 新的 settings 行 `claude_auth_watch` 只由运行中的程序写入（§5）。

**回滚：**
- **(a) 推荐：** 切换前做的数据库备份 + 旧 exe，恢复到切换前的完全相同状态。
- **(b) 只换回旧 exe：** v2 用自己保留的状态继续运行，但有三个后果：
  - v3 期间分析过的快照不会被 v2 重新分析（v2 的派生数据在这段有缺口）；
  - 质量列保持 v3 的值；
  - 每周窗口空闲时，v2 的卡住问题会重新出现。
- 切换生产版本本身属于 Stage D，需要用户明确同意，并先完成备份。

## 5. P1：Claude 退出登录通知（§8）

### 5.1 行为

| §8 要求 | 实现 |
|---|---|
| Claude monitoring enabled | 只在 Claude 监控启用、未暂停时运行环境检查；停用时不检查也不通知 |
| 此前已确认 authenticated | 状态机初始为 "从未登录"，只有看到明确的 `loggedIn: true` 才进入 "已登录" |
| 当前成功取得明确 logged-out 状态 | 只有 `claude auth status --json` 明确返回 `loggedIn: false` 才算。字段缺失、不是布尔值、输出不是 JSON、命令失败，都算 "无结果"（新增 `logged_in_reported` 保存原始值） |
| 不是已通知过的同一 episode | 状态持久化在 settings 表（`claude_auth_watch`）：已登录 → 退出通知一次；退出 → 重启 → 仍退出，不重复；退出 → 登录 → 退出，再通知一次 |
| 不误报网络中断、timeout、429、missing usage lines、parser error、unknown auth | 这些都不是登录检查的结果，不进入状态机。测试覆盖：缺字段、命令失败、`/usage` 429、`/usage` 无额度行 |
| 安全（§8.4） | 只读取 `claude auth status` 的输出（每小时一次，Phase 3.5 起即如此）；不读取凭据或 token，不修改 Claude 设置，不触发登录 |

**文案：**
- 中文标题 "Claude 监控已中断"，正文 "Claude Code 已退出登录，请重新登录。"
- 英文标题 "Claude monitoring stopped"，正文 "Claude Code is signed out. Sign in again to resume monitoring."
- 合并进其他通知时，使用完整的一句话。
- 优先级最高（`MONITORING_STOPPED`）：同时有其他提醒时，它总是标题，不会被折叠掉（§6）。
- 只有在通知写入成功后，这次 episode 才记为 "已通知"；写入失败时，下一次检查会补发（§6）。
- 检查要等 CLI 返回；如果这期间用户暂停或停用了 Claude 监控，这次不发（§6）。

**时效：** 环境检查每小时一次，所以通知最晚在退出登录后约 1 小时发出；手动刷新会立即检查。

### 5.2 升级时的行为（需要知道）

- 升级后状态机从 "从未登录" 开始。v3 第一次看到明确的已登录结果后才开始监视。
- 用户的 Claude Code 在 10-20 退出登录后已经重新登录：10-26 10:55 的只读检查返回 `loggedIn: true`（只看了登录状态字段），QuotaDock 的状态栏桥接当天 10:10 也记录到了额度数据。
- 所以切换到 v3 后的第一次检查就会看到 "已登录"，监视立即生效。
- 如果升级时恰好处于退出状态，那一段不会补发通知（不在旧 episode 上补发 "已退出"）；之后登录、再退出会通知。

### 5.3 测试

| 测试 | 旧代码 | 新代码 |
|---|---|---|
| `claude_auth` 单元测试 6 项：每个 episode 一次、从未登录不算、未知结果不改变状态、只认明确的 `false`、重启后 episode 保持、文案 | 新模块 | ✅ |
| `signing_out_after_a_confirmed_sign_in_notifies_once_per_episode`：登录 → 退出（通知）→ 再检查（不通知）→ 重启（不通知）→ 登录 → 退出（再通知）；只运行了 `--version` 和 `auth status --json` | ❌（从不通知） | ✅ |
| `no_sign_out_notice_without_an_explicit_sign_out_after_a_confirmed_sign_in`：从未登录、缺字段、命令失败、监控停用、`/usage` 429、`/usage` 无额度行 | ✅（旧代码本来就从不通知；这是防误报测试） | ✅ |
| `a_sign_out_notice_that_could_not_be_recorded_is_sent_at_the_next_check`（§6） | ❌（`fb8c32a`：通知丢失） | ✅ |
| `no_sign_out_notice_when_monitoring_stops_during_the_check`（暂停 / 停用两种，§6） | ❌（`fb8c32a`：仍然发出） | ✅ |
| `the_sign_out_notice_is_never_folded_away`（§6） | ❌（`fb8c32a`：被折叠） | ✅ |

## 6. 独立代码审查（2026-10-26）`[LOCAL TEST]`

**方式：** 在切换生产版本之前，对 `6be0d1c` 和 `fb8c32a` 做了一次独立审查（Claude Code 的多 agent workflow）：
- 3 个审查者各看一块：reset-detector-v3 状态机、Claude 退出登录通知、classifier 升级与通知去重。只读代码，不运行应用、不接触生产数据。
- 每条发现交给一个验证者。验证者在独立的 git worktree 中先试图反驳；反驳不了，就写测试复现。只有复现成功才算成立。
- 每个区域最多验证 3 条。detector 区域的第 4 条没有单独验证（"带大幅下降的提前 rollover 被记为提前刷新"），它被下面第 1 条的修复一起覆盖了。

**结果：** 8 条发现中，6 条复现成立，2 条被否定。

| # | 区域 | 发现 | 结论 | 处理 |
|---|---|---|---|---|
| 1 | detector | backend 时钟比本机快、在本机 reset 时间前几十秒就切换窗口，而这几十秒里正好有 3 次读取（一次定时 + 两次手动刷新，最小间隔 10 s）时，`BecameIdle` 会在旧 reset 时间之前确认。5 小时的自然 rollover 变成 "提前刷新" 通知；每周窗口在噪声范围内时，则完全没有周期边界。v2 对同一序列记为自然 rollover，所以这是退化 | 成立；场景很窄（5 分钟轮询不会出现） | `4204f0e`：在旧 reset 时间之前 `sliding_anchor_tolerance_secs`（120 s）以内、或已过 reset 时间的空闲读数，都是 rollover；待确认的空闲刷新（`BecameIdle` / 大幅 `QuotaRefresh`）进入这个范围时转为 rollover |
| 2 | detector | "被取代" 安全阀在第 13 次读取时无论内容如何都会重定参照点；如果这次读取正好是真实的刷新或 rollover，它只被记成诊断事件，周期边界丢失（违反 §5.5） | 成立 | `4204f0e`：安全阀只对 schedule change / unknown 候选生效；刷新、rollover、空闲候选总是照常打开 |
| 3 | Claude | 退出登录的 episode 先保存为 "已通知"，然后才写入通知。中间任何错误（例如数据库写入失败）都会让这条通知永久丢失 | 成立；需要数据库写入失败才会发生 | `c38a9d0`：通知写入成功后才提交状态；写入失败时，下一次检查仍会补发 |
| 4 | Claude | `claude_tick` 在等待 `claude auth status` 之前检查启用 / 暂停；如果用户在等待期间暂停或停用了 Claude 监控，仍会发出通知 | 成立；竞态窗口只有几秒 | `c38a9d0`：检查结束后再判断一次 |
| 5 | Claude | 合并通知时，正文最多再列 2 条，但所有规则都会记为已发送。同时有 3 条以上其他提醒时，退出登录通知会被折叠掉而用户看不到 | 成立；默认设置下不会出现 | `c38a9d0`：退出登录通知优先级最高，总是标题。合并逻辑的通用问题写入 backlog 第 33 项 |
| 6 | 升级 | v3 可能为同一段真实时间给出不同的周期键；v2 在这段时间已发出的阈值 / 预测提醒会在升级后重发 | 机制成立（验证者在同一数据库上先后运行 v2 和 v3 复现） | 不影响本次升级（见下）；写入 backlog 第 34 项 |
| — | detector / 升级 | 空闲刷新与 reset credit 减少相关时，立即记为 credit 兑换 | 否定：这是已有的强证据规则（v2 对大幅刷新也是如此），不需要 3 次空闲读数；有专门的测试 | 无 |
| — | 升级 | 同上，从 "3% → 0% 归因于 reset credit" 的角度提出 | 否定：同上 | 无 |

**第 6 项为什么不影响本次升级：** 重发需要同时满足两个条件：
- v2 在这段时间里发过提醒；
- 升级时剩余量仍低于阈值。

本库这段时间（每周 10-23 之后的周期）正是 v2 卡住的时段：快照都被标为 suspect，阈值评估暂停，v2 没有发出任何阈值或预测提醒（§4.3 的 18 条记录中没有这个周期的）。所以 v3 在这段时间发出的第一条提醒都是首次，不是重发。另外，V1 尚未发布，没有其他用户持有 v2 数据库。

**修复的验证：**
- 新增 7 项回归测试，每项都在修复前的提交上失败、修复后通过：
  - `detector_v3.rs` 4 项：`a_rollover_seen_early_with_fast_polls_is_still_a_rollover`（4% 与 35% 两种）、`a_weekly_rollover_seen_early_within_noise_closes_the_cycle`、`a_refresh_on_the_poll_that_ends_a_superseded_run_is_kept`（30% 与 8%）、`a_rollover_on_the_poll_that_ends_a_superseded_run_is_kept`；
  - `claude_provider.rs` 3 项：见 §5.3。
- 修复后的 v3 重分析 10,609 个真实快照，结果与修复前逐字节相同（sha256 `43ce7eaa…`）。也就是说，真实历史里没有落在 reset 前 120 s 以内的空闲读数，也没有长到 12 次的被取代序列。§4 的数字不变。

## 7. 全量回归 `[LOCAL TEST]`

| 检查 | 结果 |
|---|---|
| `cargo test --workspace` | `c38a9d0`：188 项通过：lib 40，analytics 8，claude_provider 30，detector_v2 6，detector_v3 24，engine_scenarios 25，parity 2，polling 7，product 22，rpc_and_provider 9，soak_hardening 3，storage 12（`814ee9c` 为 181 项，Stage A 前为 152 项） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 无警告（`814ee9c` 和 `c38a9d0`） |
| rustfmt | 本阶段改动的文件都符合 `rustfmt.toml`。仓库中其他文件原有的格式差异没有改动（§9） |
| UI | `tsc --noEmit` 通过；`node --test` 24 项通过 |
| 根目录 TypeScript 参考测试 | `tsc --noEmit` 通过；`node --test "tests/**/*.test.ts"` 83 项通过（Stage B 开始时补跑，代码与 `814ee9c` 相同） |
| 中间提交 | `6be0d1c` 单独检出状态下 `cargo test -p quotadock-core` 全部通过 |

## 8. 写入 backlog 的事项（§9）

详见 `POST_V1_BACKLOG.md` 第 26–34 项：

26. 固定重置时间的 1 秒抖动在前台 60 s 节奏下反复触发 schedule change 和 reconciliation（10-19 15:22–16:10 的真实例子），少量快照被标为 suspect。不对用户可见，v2 / v3 相同。
27. 大幅下降的 `QuotaRefresh` 若在重置边界因时钟偏差提前出现，确认前旧重置时间已到，仍会记为提前刷新（v2 行为；`BecameIdle` 已处理此情况）。真实数据中未出现。
28. 小幅提前刷新后不到 3 次轮询就恢复使用，记为诊断性的 `unknown_quota_transition`，不显示为提前刷新。真实数据中未出现。
29. classifier 升级时，启动阶段的重分析会阻塞启动：10,609 个快照约 2.4 s，随历史线性增长。
30. Windows 关机时没有写 `app_stop`（Core soak §8.2）。
31. 主窗口长时间打开时 CPU 缓慢上升（Core soak §8.4）。
32. 离线 soak 报告的分类问题（Core soak §8.5）。
33. 合并通知时被折叠的规则也记为已发送（§6 第 5 项的通用部分）。
34. 以后的 classifier 升级应按时间迁移阈值 / 预测提醒的去重记录（§6 第 6 项）。

## 9. 生产与授权边界

- **生产 QuotaDock：** 本阶段没有启动、停止或替换。生产 exe 仍是第 2 轮的 `D5CCD2C9…`。
- **生产数据库：** 只通过只读连接做了一次在线备份（§4.1），没有写入，也没有迁移。
- **Claude 与 Codex：** 没有修改 Claude 设置，没有读取凭据；工具没有启动任何真实 provider 连接。
- **Git：** 没有推送 remote；没有改写历史，没有移动 soak baseline tag。

## 10. 下一步

Stage B 已完成，见 `WINDOWS_V1_WIDGET_INTEGRATION_FINDINGS.md`（`windows-v1-rc` 分支）：
- 热修复和审查修复都已 cherry-pick 到 `phase37-widget`；
- `windows-v1-rc` 合并了两边，版本 `1.0.0-rc.2`。

**需要用户决定的事（Stage D 之前）：**
- 是否同意把生产版本切换为 RC：切换前会记录当前二进制哈希、确认数据库路径并做好备份，回滚见 §4.4。
- 是否开启 "Windows 启动时运行 QuotaDock"。
- Claude Code 已重新登录（§5.2），这一项已满足。
