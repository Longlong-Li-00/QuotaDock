# QuotaDock Windows V1 Widget Integration Findings

Windows V1 · Final Integration Stage B（同步 Widget 分支、建立 RC）与 Stage C（全量回归、Windows 实机验证）· branch `windows-v1-rc` · report date 2026-10-26 · Windows 11 Home China 10.0.26200，单显示器 1920×1080（100%）

依据标记：
- `[LOCAL TEST]`：自动化测试。
- `[REAL WINDOW]`：在用户桌面上运行的 RC 隔离实例（临时数据库、fake Codex、fake Claude CLI），由 UI Automation 操作，或由用户亲手操作、工具记录。
- `[CODE]`：代码与配置审阅。
- `NOT VERIFIED`：没有足够的真实证据，不写 PASS。

## 结论

**Stage B、Stage C 完成。** 当前 RC 为 **`1.0.0-rc.3`**（`3640b19`），Stage D 在用它运行；变化见 §7。

rc.2（`fc98c92`）是本报告 §1–§6 验证的版本。rc.3 只在它的基础上把小组件改为 **双击** 打开主窗口（用户在 Stage D 中提出）。

- **同步与集成（Stage B）：**
  - Core 修复以 cherry-pick 同步到 `phase37-widget`，没有冲突；
  - `windows-v1-rc` 合并了 baseline 和 Widget 分支，只有一处文档冲突。
- **独立代码审查：** 切换前对 Core 热修复做了一次独立审查，复现成立的问题中 5 个已修复，于是 RC 从 rc.1 升到 rc.2（见 `PHASE38_CORE_HOTFIX_FINDINGS.md` §6）。
- **自动化回归（§16）：** 三个分支全部通过。RC 有 214 项 Rust 测试、clippy、UI 43 项、TypeScript 参考测试 87 项。
- **Windows 实机验证（§17）：** 隔离实例中，Widget、Core、生命周期的检查项全部通过，其中包括：
  - **v3 提前刷新的现场复现**：真实 RC 二进制、真实轮询节奏，结果与规格 §5.2 完全一致；
  - **§15 不变量**：小组件显示、主窗口隐藏时，Codex 仍按 300 s 后台节奏读取。
- **未验证（NOT VERIFIED）：** 真实 provider 数据下的行为、Windows 注销 / 关机、正式安装后的 toast 身份、真实第二显示器。它们分别留给 Stage D / E，或在 §5.4 说明原因。

---

## 1. Git 记录（§11、§12）`[CODE]`

| 项 | 提交 | 说明 |
|---|---|---|
| Soak baseline tag | `fab9a0e`（`windows-v1-soak-baseline-20261011`） | 未移动 |
| Hotfix commits（baseline） | `6be0d1c` reset-detector-v3；`fb8c32a` Claude 退出登录通知 | `PHASE38_CORE_HOTFIX_FINDINGS.md` §1–§5 |
| Review fixes（baseline） | `4204f0e` detector；`c38a9d0` Claude 通知 | 同上 §6 |
| Cherry-picks（widget） | `3aaa3d5` ← `6be0d1c`；`85bb75b` ← `fb8c32a`；`82b777c` ← `4204f0e`；`5ea3fc9` ← `c38a9d0` | 都用 `git cherry-pick -x`，没有冲突 |
| Base commit | `c38a9d0`（`v1-soak-baseline` 上最后一个代码提交） | 之后的 baseline 提交只改文档 |
| Widget commit | `5ea3fc9`（`phase37-widget`） | Phase 3.7 小组件、历史图表改动，以及上述 4 个修复 |
| Merge commits | `1cb5419`（首次合并 widget）；`98d2b70`、`a888504`（合并 baseline）；`f48bf94`（合并 widget） | 全部 `--no-ff`，合并方向只有 baseline → RC、widget → RC |
| Version commits | `5f1cf7f` `1.0.0-rc.1`；`fc98c92` `1.0.0-rc.2`；`3640b19` `1.0.0-rc.3` | rc.1、rc.2 已被取代（§7） |

**分支规则（§11）：**
- 没有把 Widget 分支合并回 baseline。
- 同步到 Widget 分支的只有纯 Core 提交（热修复 2 个 + 审查修复 2 个）。
- 文档提交只经 RC 合并进入 RC。

**合并冲突：** 只有第一次合并时的 `SOAK_BASELINE_MANIFEST.md`。baseline 新增了一节，Widget 分支的版本是它的子集，于是采用 baseline 的版本。

**结果核对：**
- `fc98c92` 与 `phase37-widget`（`5ea3fc9`）的源码（`crates`、`src-tauri/src`、`ui/src`、`src`、`scripts`、`tests`）只差 `crates/quotadock-core/Cargo.toml` 中的版本号。
- 其余差别都是文档和版本文件。

## 2. RC 二进制

| 版本 | 文件 | 大小 | SHA-256 |
|---|---|---|---|
| **1.0.0-rc.2**（`fc98c92`） | `quotadock-app.exe` | 16,444,416 | `19B33C16D3C693A6A6C62647A04843362DAB2E25BD41971C8D0B5D5CEF690351` |
| | `quotadock-claude-bridge.exe` | 2,010,112 | `E819EC816DE22F2E9E45334924112CFD02968F561103F527ED3ED0E853173B48` |
| 1.0.0-rc.1（`5f1cf7f`，已取代） | `quotadock-app.exe` | 16,454,656 | `6EBFD919DF36F66AFD7AA143EFEDA7FF1FAC210E0402B13388D757B4D25AC5A2` |
| | `quotadock-claude-bridge.exe` | 2,010,112 | `742ABF7D05570A5294D8BE14E7DCCC7584418846B506664ED0059D41393A6317` |

**构建方式：** 在 RC worktree（`.worktrees/windows-v1-rc`）中：
1. 构建前端：`tsc --noEmit` 和 `vite build`，产出 `index.html`、`widget.html`；
2. `cargo build --release -p quotadock-app`；
3. `cargo build --release -p quotadock-core --bin quotadock-claude-bridge`。

输出在该 worktree 的 `target\release`。这不是安装包，也没有签名（Stage E）。生产使用的第 2 轮二进制（主树 `target\release`，`D5CCD2C9…`）没有被覆盖。

**版本（§37）：** 以下各处统一为 `1.0.0-rc.2`：Cargo（core、app，以及 app 对 core 的依赖声明）、`tauri.conf.json`、`ui/package.json`、根目录 `package.json`、两个 lock 文件。

程序在 `app_start` 健康记录、诊断导出（§5.2 已确认）和 Codex `clientInfo` 中报告这个版本。没有改的两处：
- `health.rs` 单元测试里的样例数据；
- Phase 1 的 TypeScript 原型 `src/codex/transport/transport.ts`（不随产品发布）。

## 3. 自动化回归（§16）`[LOCAL TEST]`

| 检查 | baseline `c38a9d0` | widget `5ea3fc9` | RC `fc98c92` |
|---|---|---|---|
| `cargo test --workspace` | 188 通过 | 214 通过 | 214 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 无警告 | 无警告 | 无警告 |
| UI `tsc --noEmit` | 通过 | 通过 | 通过 |
| UI `node --test "src/**/*.test.ts"` | 24 通过 | 43 通过 | 43 通过 |
| 根目录 `tsc --noEmit` | 通过 | 通过 | 通过 |
| 根目录 `node --test "tests/**/*.test.ts"`（TypeScript reference） | 83 通过 | 87 通过 | 87 通过 |

RC 的 214 项 Rust 测试：lib 40，analytics 8，claude_provider 30，detector_v2 6，detector_v3 24，engine_scenarios 25，parity 2，polling 7，product 23，rpc_and_provider 9，soak_hardening 3，storage 12，widget 25。

§16 列出的类别对应：

| 类别 | 测试 |
|---|---|
| Database migration | `storage.rs`：`migrations_pragmas_and_idempotent_reopen`、`only_one_engine_per_database`、`foreign_keys_set_null_on_snapshot_retention_delete` |
| Reanalysis | `storage.rs`：`reanalysis_reproduces_the_same_derived_history`、`a_new_classifier_reanalyses_history_and_keeps_earlier_rows`；`product.rs`：`early_refresh_notifies_once_live_and_a_classifier_replay_never_resends_it`；`detector_v3.rs`（真实 fixture）；生产拷贝上的完整重放（热修复报告 §4，审查修复后结果不变） |
| Widget | `widget.rs` 25 项，加上 UI 中的 widget 测试 |
| Notification | `product.rs`（阈值去重、聚合、诊断事件不通知、提前刷新只通知一次）；`claude_provider.rs`（Claude 提醒、退出登录通知 5 项）；`claude_auth` 单元测试 6 项 |

## 4. Widget 要求核对（§13–§15）

### 4.1 功能（§13）

Phase 3.7 已开发的功能全部保留（实机结果见 §5.2）。§13 "不再加入" 的项目一项都没有加入（backlog 第 16–23 项）。

### 4.2 只负责显示（§14）`[CODE]` `[LOCAL TEST]` `[REAL WINDOW]`

- **`widget_uses_product_state_only`：** 小组件的数字和严重程度与托盘模型一致。
- **`widget_does_not_trigger_provider_refresh`：** 读取视图 50 次，Codex 读取 0 次、Claude CLI 调用 0 次。
- **小组件页面的权限（`src-tauri/capabilities/widget.json`）：** 只有 4 个命令：读取视图、打开主窗口、开始拖动（锁定时由后端拒绝）、显示菜单。
- **实机：** 小组件的显示与托盘、主窗口一致（Codex 73% / 59%、Claude 88% / 29%，都是 fake 数据）。阈值标记 "!" 来自后端的严重程度。

### 4.3 不改变轮询节奏（§15）`[LOCAL TEST]` `[REAL WINDOW]`

- **自动化测试：** 前台状态 16 种组合；Codex 300 s 后台节奏；Claude 不进入 5 分钟前台间隔。
- **实机（fake Codex 记录每次请求）：** 以 `--hidden` 启动，主窗口隐藏、小组件显示（11:25:52 起），只有两次用户操作：用户 11:29:01 打开了主窗口（打开即读取一次，设计行为），11:29:52 又关闭。
  - 此后的读取正好在 11:34:01，即 300 s 后；
  - 到 11:37:16 测量结束都没有再读取；
  - 另一段主窗口隐藏期间（11:19:20–11:23:46），265 s 内没有读取；
  - 主窗口可见时为 60 s 节奏（11:38–11:44，每分钟一次）。

## 5. Windows 实机验证（§17）`[REAL WINDOW]`

### 5.1 环境

| 方面 | 做法 |
|---|---|
| 程序 | RC 二进制，从 RC worktree 的 `target\release` 运行。rc.1 用于 10:55–11:50 的大部分检查；审查修复后用 rc.2 重做了 v3 现场复现、诊断导出、退出 |
| 数据 | `QUOTADOCK_DATA_DIR` 指向 `target\rc-validation\data`（以及现场复现用的 `data-v3live`）；生产数据库没有被打开（主文件、WAL 的大小和修改时间不变） |
| Codex | `QUOTADOCK_CODEX_PATH` 指向一个只为验证写的 fake `codex.exe`（在 gitignored 的 `target\rc-validation\fake-codex`，不进仓库）：回答 `--version`，按 app-server 协议返回 `state.json` 中的额度，可模拟空闲窗口，记录每次请求 |
| Claude | 仓库的 `fake-claude` 改名为 `claude.exe`，放在启动器的 `PATH` 中（同时调整 `APPDATA` / `USERPROFILE`，让程序找不到真实的 Claude Code） |
| 启动 | 环境变量写在 `.cmd` 启动器里，由 Explorer 启动 |
| 无法隔离的部分 | 状态栏桥接文件按真实用户目录读取，所以第一个数据点是用户 10:10 的真实 Claude 记录（只进入临时库）；单实例标识；toast 身份；自启动注册表没有切换；Claude statusLine 设置没有改动 |

**测试环境本身的影响：**
- **fake 数据触发的测试通知：** 共 4 条，都是有意触发、用户事先知情：
  - 10:55 和 12:02 的 "Claude 每周额度低于 30%"；
  - 11:29 的 "Codex 每周额度低于 30%"；
  - 12:07 的 "Codex 额度提前刷新（每周）"。
- **WebView2 分成两组进程（12 个）：** 原因是启动器改了 `USERPROFILE`，而 LocalAppData 的系统路径按 `%USERPROFILE%` 展开。程序首次启动时假目录还不存在，主窗口的 WebView2 数据目录就退回到了 exe 旁边。之后假目录已存在，重启后主窗口与小组件共用一个数据目录、一组 7 个进程。生产环境的 WebView2 数据目录一直是 `%LOCALAPPDATA%\com.quotadock.app\EBWebView`（第 2 轮 soak 留下的文件证实了这一点），所以不会出现这个问题。

### 5.2 结果

**Widget**

| §17 项目 | 结果 | 依据 |
|---|---|---|
| Enable / Disable | ✅ | 设置中打开：创建 1 个窗口（默认关闭、默认右上角 16 px 边距、置顶）；关闭：窗口销毁；用户亲手关、开各一次 |
| Drag | ✅ | 用户拖动多次；窗口状态记录器（每 200 ms）记录了位置随鼠标变化 |
| Lock / Unlock | ✅ | 锁定期间（两段）位置一次都没变；解锁后可以拖动；也可通过右键菜单切换 |
| Remember position | ✅ | 关闭再打开小组件：`restored=true`；"记住位置" 打开时重启恢复到 (623, 18)；关闭时重启回到默认位置 (1680, 16) |
| Restart | ✅ | 退出后重新启动：小组件随启动出现，布局、置顶、透明度、锁定设置都保留 |
| Always-on-top ON / OFF | ✅ | 窗口 `WS_EX_TOPMOST` 随设置变化；关闭时可被最大化的窗口遮挡 |
| Compact / Detailed | ✅ | 用户切换了两次；紧凑 224×102 |
| 50% / 85% / 100% opacity | ✅ | 85%（默认）、50%、100% 以及 55%、65% 都应用到了小组件（健康日志 `updated` 记录对应值） |
| Light / Dark | ✅ | 浅色、深色截图：小组件和主窗口同时切换；之后恢复 "跟随系统" |
| Chinese / English | ✅ | English 截图（"5h / Week"、整页设置为英文）；之后恢复 "跟随系统" |
| Main close-to-tray | ✅ | 关闭主窗口：窗口隐藏，进程和小组件继续运行（`foreground false`） |
| Widget remains visible | ✅ | 主窗口隐藏期间，小组件一直可见、置顶 |
| Widget opens Main Window | ✅ | 在小组件标题上单击（受控的一次真实鼠标点击，不移动）：主窗口重新显示并获得前台。用户先前的一次尝试移动超过 4 px，被当作拖动 |
| Single Instance | ✅ | 再次启动：第二个进程在 250 ms 内退出，已运行实例的主窗口被唤出；小组件始终只有 1 个 |
| Multi-monitor fallback | ✅（模拟） | 保存的位置改为不存在的 `DISPLAY2`（x=2600）：启动时 `recovered=true`，小组件回到主屏默认位置。没有真实的第二显示器 |
| 原生右键菜单 | ✅ | 菜单弹出两次（`#32768` 窗口），通过菜单切换了锁定 |
| 无任务栏按钮 | ✅ | 任务栏只有 "QuotaDock - 1 个运行窗口"（主窗口） |

**Core**

| §17 项目 | 结果 | 依据 |
|---|---|---|
| Codex snapshot | ✅（fake） | 启动、定时、手动读取都写入快照并显示 |
| Claude snapshot | ✅（fake） | `claude --version`、`auth status --json`、`-p /usage` 由 fake CLI 回答并显示；退出登录监视在第一次检查时进入 "已登录" |
| Pause / Resume | ✅ | 暂停 72 s 内读取 0 次，app-server 被停止；恢复后立即读取一次并启动新的 app-server。小组件在 5 s 内显示 "已暂停"（暂停时先等 app-server 退出，再刷新小组件） |
| Manual refresh | ✅ | 立即读取一次；距上次读取不到 10 s 时被限流（`MIN_MANUAL_GAP`，设计行为） |
| Automatic refresh | ✅ | 前台 60 s，后台 300 s（§4.3） |
| Notifications | ✅ | 4 条测试通知全部送达（`delivered = 1`）。文案正确，包括 v3 提前刷新的新文案 |
| v3 提前刷新现场复现（P0） | ✅ | 见下表 |
| History | ✅ | 历史页显示 Codex 和 Claude 的 5 小时、每周曲线（小倍图） |
| Events | ✅ | 现场复现的刷新事件出现在事件记录中（数据库和导出里都有） |
| SQLite | ✅ | 两个测试数据库 `integrity_check: ok` |
| Diagnostics export | ✅ | 写入数据目录的 `exports\diagnostics-*.json`（28 KB），包含 `appVersion 1.0.0-rc.2`、`classifierVersion reset-detector-v3`；扫描不到账号 id、邮箱、用户路径或任何 token |
| Provider process cleanup | ✅ | 每次退出后：fake `codex.exe`、它的 `conhost.exe`、全部 WebView2 进程都结束；暂停时 app-server 也被停止 |

**v3 提前刷新现场复现**（rc.2，全新数据库，12:02–12:08，fake Codex 先报每周 3%、固定 reset，之后报空闲）：

| 读取 | 每周 | 显示的 "已确认 reset" | 候选 |
|---|---|---|---|
| #1 | 3%，reset 10-29 12:02 | 无（还只有一次） | 无 |
| #2、#3 | 同上 | 10-29 12:02 | 无 |
| #4、#5 | 0%，reset = 现在 + 7 天 | **无**（旧 reset 不再显示） | `became_idle` 待确认 |
| #6 | 同上 | 无 | 确认：`unscheduled_quota_refresh`，每周，user，`medium`，`cause: unknown`，#3 → #6 |
| #7 | 同上 | 无 | sliding anchor，无新事件 |

- 通知（12:07:30，已送达）：标题 "Codex 额度提前刷新 (每周)"，正文 "观察到 Codex 每周额度提前刷新，具体原因未知。"
- 周期：#1–#3 由 `unscheduled_quota_refresh` 关闭，从 #4 开始新周期。只有一个边界。
- 全部事件：5 小时 sliding 诊断 1 条，每周刷新 1 条。

**Lifecycle**

| §17 项目 | 结果 | 依据 |
|---|---|---|
| Normal quit | ✅ | 设置页 "退出 QuotaDock"：约 4 s 内退出；健康日志依次记录小组件 `destroyed (quit)`、`app_stop (quit)` |
| Restart | ✅ | 见 Widget 表 |
| Startup behavior | ✅ | `--hidden`（与 "Windows 启动时运行" 相同的参数）：`launch: hidden`，主窗口不显示，小组件显示 |
| No orphan processes | ✅ | 每次退出 2 s 后，QuotaDock、fake 程序、WebView2 进程都是 0 |
| Windows logout / shutdown | **NOT VERIFIED** | 不能在用户正在使用的会话中注销或关机。已知第 2 轮 soak 中 Windows 关机时没有写 `app_stop`（backlog 第 30 项），留给 Stage D 观察 |

### 5.3 资源基线（§18）

fake 数据，每个状态测 3–11 分钟，每 10 s 一次。这只是短时间的快照，不能用来判断泄漏；长期趋势在 72 小时 soak 中测量。

| 状态 | 样本 | QuotaDock private | QuotaDock 工作集 | 句柄 | QuotaDock CPU | WebView2 进程 | WebView2 private | WebView2 工作集 | WebView2 CPU |
|---|---|---|---|---|---|---|---|---|---|
| A 小组件关 / 主窗口隐藏 | 18 | 16.3 MB | 52.2 MB | 427 | 0.02% | 6 | 182 MB | 400 MB | 0.04% |
| B 小组件开 / 主窗口显示 | 18 | 16.1 MB | 50.2 MB | 432–438 | 0.08% | 7 | 212 MB | 451 MB | 0.75% |
| **C 小组件开 / 主窗口隐藏（主要运行状态）** | 63 | 14.3 MB | 46.6 MB | 413–446 | 0.06% | 7 | 203 MB | 444 MB | 0.35% |
| 参考：小组件关 / 主窗口显示 | 18 | 16.4 MB | 52.2 MB | 427–433 | 0.07% | 6 | 190 MB | 407 MB | 1.37% |

- CPU 为单核百分比。
- 数据库：主数据库加 WAL 不到 1 MB。
- 小组件只多一个渲染进程，约 +20–30 MB private、+40 MB 工作集；退出后全部释放。

### 5.4 没有验证的项目

| 项目 | 原因 | 何时验证 |
|---|---|---|
| 真实 Codex / Claude 数据下的 RC | 需要切换生产版本 | Stage D（须用户同意） |
| Windows 注销 / 关机 | 不能在用户会话中进行 | Stage D 中如自然发生则记录 |
| toast 显示为 "QuotaDock" 的应用身份 | 开发版本没有注册 AUMID | Stage E（安装包） |
| 真实第二显示器 | 本机只有一个显示器 | 只模拟了 "显示器不存在" 的恢复 |
| Claude 退出登录通知的真实 toast | 规格 §24：不为验证而退出用户的真实账号 | 如在 Stage D 中自然发生则记录；否则保持 NOT VERIFIED |

## 6. 下一步：Stage D

需要用户决定：

1. **是否同意把生产版本切换为 `1.0.0-rc.2`。** 切换前会：
   - 记录当前生产二进制的 SHA-256（`D5CCD2C9…`）；
   - 确认数据库路径（`%LOCALAPPDATA%\QuotaDock\quotadock.sqlite3`）；
   - 做一次在线备份，并核对备份的 `integrity_check`；
   - 说明迁移与回滚：首次启动时用 v3 重分析约 3 s，v2 数据保留；回滚 = 备份 + 旧 exe（热修复报告 §4.4）。
2. **是否开启 "Windows 启动时运行 QuotaDock"：** 第 1 轮 soak 因为没有自启动损失了约 45 小时。

Claude Code 已重新登录（10-26 10:55 只读检查），Claude 覆盖的前提已满足。

## 7. rc.3：双击打开主窗口（2026-10-26，Stage D 中用户提出）

**原因：** 用户反馈误触小组件就会打开主窗口。规格 §13 原来写的是 "Click to open Main Window"，用户决定改为双击，并接受因此从 rc.3 起重新计算 72 小时。

**改动（只在 widget 页面）：**
- 提交 `3262dd5`（`phase37-widget`），经 `windows-v1-rc` 合并，版本提交 `3640b19`。
- 在标题或某一行上 **双击** 才打开 QuotaDock，双击间隔跟随 Windows 设置；单击不做任何事。
- 按住移动仍然是拖动（锁定时由后端拒绝），右键菜单 "打开 QuotaDock" 和键盘 Enter / Space 不变。
- 设置页说明改为 "拖动可移动 · 双击打开 QuotaDock · 右键查看选项"（英文 "Double-click to open"）。
- Rust 代码没有变化。

| 文件 | 大小 | SHA-256 |
|---|---|---|
| `quotadock-app.exe`（rc.3） | 16,444,416 | `E7942DEE56557FC2B700B3331EB2E403C9134C91CECA9F22D8CE42FA88BD3C00` |
| `quotadock-claude-bridge.exe`（rc.3） | 2,010,112 | `A3C3E83234CAF728354172594E2AA69BC3126E8E3416E17FB4F957B02C20346C` |

**验证：**
- 自动化：Rust 214、clippy、UI 43、TypeScript 参考测试 87，全部通过；构建出的前端确认包含新的处理和文案，不再有 "单击打开"。
- 浏览器预览（mock）：单击后没有打开请求，双击后一次。
- 生产小组件（rc.3，真实窗口，受控的真实鼠标点击）：单击后 2 s 主窗口仍隐藏；双击后主窗口显示。
