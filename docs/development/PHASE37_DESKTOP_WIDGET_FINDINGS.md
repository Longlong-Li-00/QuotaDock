# QuotaDock Phase 3.7 Desktop Widget Findings（开发阶段）

Windows V1 · 分支 `phase37-widget` · 2026-10-11 · Windows 11 Home China 10.0.26200

本文件在开发过程中维护（PLAN A §86）。目前处于 **STEP 6：只用 mock / 临时数据库 / 测试数据开发**。这期间正在运行的 7 天 Core soak 不受影响。真实运行、资源测量和 72 小时 Widget soak 都要等 Core GO 之后进行。

依据标记：
- `[LOCAL TEST]`：自动化测试（临时数据库、fake provider）。
- `[MOCK PREVIEW]`：浏览器中用 mock 数据渲染的真实 Widget 页面。
- `[PENDING REAL RUN]`：需要 Core GO 后真实运行验证。
- `[QUOTADOCK HEURISTIC]`：QuotaDock 自定的规则。

## 结论速览

| 项目 | 结果 |
|---|---|
| 与当前 soak 隔离 | ✅ Widget 代码只在 worktree `.worktrees/phase37-widget` 中，有独立的 `target`。没有启动过 Widget build，没有访问生产数据库，Widget 开发期间没有替换或重启 soak 进程（第 1 轮 pid 60332，从 10-11 16:30:32 开始）。第 1 轮后来因 Windows 重启和 Claude blocker 修复而结束，见 `SOAK_BASELINE_MANIFEST.md` |
| 架构 | ✅ Widget 视图由 Rust 从托盘同一个 `TrayStatusModel` 生成。页面只负责显示，不计算额度、重置、预测或严重程度 |
| P0：不改变轮询节奏 | ✅ `[LOCAL TEST]` "前台" 只取决于主窗口是否显示且未最小化。Widget 显示或获得焦点时，Codex 仍是 300 s，Claude 仍是 15 min。真实验证见 §64 `[PENDING REAL RUN]` |
| P0：不产生 provider 请求 | ✅ `[LOCAL TEST]` 读取 Widget 视图不会产生 poll_runs、Claude CLI 调用或 provider session |
| P0：最多一个 Widget 窗口 | ✅ `[LOCAL TEST]` 固定 label + 生命周期规则 + 模拟器断言 `max_seen == 1`。真实验证 `[PENDING REAL RUN]` |
| 安全边界 | ✅ 独立 capability，只有 4 个 Widget 专用命令，每个命令还会检查调用窗口。主窗口的 18 个命令不变 |
| 显示规则 | ✅ `[LOCAL TEST]` + `[MOCK PREVIEW]` 22 个场景（紧凑 / 详细 × 中 / 英 × 浅 / 深 × 不透明度）。最终没有文字截断，内容高度正好 |
| 设置 | ✅ 默认关闭；6 项设置可持久保存。开发中发现并修复了一类并发写设置的问题（见 §5） |
| 测试 | ✅ Rust 176（Widget 25）、UI 34（Widget 10）、TS 参考实现 87（Widget 摘要 4），全部通过；`cargo clippy --workspace --all-targets` 零警告；UI 与根目录 typecheck 通过 |
| 资源影响 | ⏳ 未测量（按规定 Core GO 之后测量） |
| 手动验证（§63 24 步 + §64 节奏） | ⏳ Core GO 之后 |
| Widget GO / NO-GO | ⏳ 需要 72 小时 Widget soak（§65–§78） |

---

## 0. 与当前 soak 的隔离

| 项目 | 状态 |
|---|---|
| 代码位置 | Widget 开发全部在 `phase37-widget` 分支和 worktree 中；主目录停在 `v1-soak-baseline` |
| 构建 | 只在 worktree 中执行 `cargo check / clippy / test` 和 `vite build`；主目录的 `target\release`（soak exe）和 `ui\dist` 没有被改动 |
| 运行 | 没有启动过 Widget 版本的 app；没有第二个 Codex app-server；没有运行真实 Claude `/usage`；没有改动 Claude statusLine |
| 数据 | 测试只用临时数据库和 fake provider；UI 预览只用浏览器 mock（数据是固定场景，不是从生产数据复制的） |
| 预览方式 | 只读 Widget 页面单独打包为一个 HTML 文件（只有 mock），放在 worktree 被忽略的 `target\widget-preview` 中打开。没有修改主目录的 `.claude/launch.json` |

soak 期间对 baseline 分支有两次提交（都记录在 `SOAK_BASELINE_MANIFEST.md`，然后 cherry-pick 到本分支）。两次都**不需要**重新构建或重启 soak：

1. `8f9fb4e`：`ui/src/api/mock.ts` 中 mock Claude 卡片的账号指纹前缀来自真实数据，已替换为假值。只影响浏览器预览。被打 tag 的提交历史中仍然有旧值，所以**这个仓库在推送到任何远端之前，必须先改写或压缩历史**。
2. `0d275c8`：soak 报告的间隔分类修正。窗口在间隔开始时显示、随后被隐藏时，现在按托盘节奏判断。之前的 3 个 "unexplained" Codex 间隔（每个正好 300 s）和 1 个 Claude 间隔都是报告误判；程序内的 watchdog 本来就记录为 0。这只改了只读工具。

## 1. 架构

```
quotadock-core（Rust）
  product/tray.rs       TrayStatusModel（托盘与仪表盘同一个模型）
  product/widget.rs     build_view → WidgetView；以及规则：
                        decide（生命周期）、window_spec、widget_size、place / save_position（位置）、
                        provider_foreground（什么算前台）
  product/settings.rs   DesktopWidgetSettings
  product/runtime.rs    widget_view()；modify_settings / save_settings_from_page（设置写入串行化）

src-tauri（app shell，只执行规则）
  widget.rs             创建 / 更新 / 销毁、刷新提示、拖动 / 锁定、位置保存、右键菜单、健康日志
  window.rs             sync_foreground 改用 provider_foreground；Widget 窗口事件转给 widget.rs
  commands.rs           4 个 Widget 命令（only_widget 检查调用者）
  capabilities/widget.json

ui（第二个页面）
  widget.html → src/widget/main.tsx → Widget.tsx（只显示）
  src/widget/present.ts（措辞与格式）、api.ts（4 个命令）、mock.ts（浏览器预览）
```

**单一数据源。** `ProductRuntime::widget_view()` 调用与托盘相同的 `tray_with(...)`，然后由 `widget::build_view` 转换。`widget_uses_product_state_only` 测试断言 Widget 的数值和状态与 `tray_status()` 一致。Widget 页面不读 SQLite，也不计算 reset、forecast 或 severity 阈值。

**更新机制。**
- Rust 在以下时机调用 `window.__quotadockWidget.refresh()`：每次 Codex 轮询后；每次 Claude 步骤有变化后；暂停 / 恢复时；设置改变时。
- 页面收到后通过 `get_widget_view` 重新读取，这只是本地读取。
- Widget 可见时，另有一个 60 s 的保底重读（用于 "数据可能过期" 这类随时间变化的状态）。"N 分钟前" 的时钟每 15 s 只在页面内更新，不调用后端。

## 2. 窗口模型

| 属性 | 值 |
|---|---|
| label / 页面 | `widget` / `widget.html`（label 唯一，Tauri 不允许两个同名窗口） |
| 外观 | 无边框、透明、无阴影、不在任务栏、不可调整大小、不可最大化 / 最小化 |
| 置顶 | 由设置决定（默认开启），可以在运行中切换 |
| 焦点 | 创建时 `focused(false)` + `visible(true)`，即 `SW_SHOWNOACTIVATE`，不抢用户的焦点。之后不再调用 `show()`，因为 tao 的 `show()` 会激活窗口 |
| 尺寸（逻辑像素） | 紧凑 224 × (58 + 22n)，详细 224 × (70 + 64n)，n = provider 数（1–2） |
| 创建线程 | 在 async runtime 上创建，避免 Windows 上从主线程事件处理器创建窗口导致死锁 |
| 关闭 | 用 `destroy()` 而不是 `close()`（`close()` 会触发 "用户关闭了 Widget" 的处理） |
| 不透明度 | 只作用于 CSS 背景，文字始终不透明 |

**位置。**
- **保存：** 拖动结束 700 ms 后（只有开启 "记住位置" 时），保存显示器名称加相对工作区原点的逻辑偏移，这样 DPI 变化后仍然正确。
- **恢复：** 先按显示器名称找到原来的显示器，然后把位置限制在它的工作区内。
- **回退：** 如果该显示器已不存在，或者没有保存过位置，就放到主显示器工作区的右上角，距离边缘 16 px（逻辑像素）。
- **记录：** 恢复和回退都写入健康日志的 `created` 事件（`restored` / `recovered`）。

## 3. 生命周期

规则在 `widget::decide`，app shell 只负责执行。

| 事件 | 设置开启 | 窗口存在 | 动作 |
|---|---|---|---|
| Startup | 是 | 否 | Create（autostart / 隐藏启动也一样） |
| SettingsChanged | 是 / 否 | 否 / 是 | Create / Update / Destroy / Keep |
| WidgetClosed（Widget 上按 Alt+F4） | — | 是 | Destroy，并把设置改为关闭；QuotaDock 和监控继续运行 |
| Paused / Resumed | — | 是 | Refresh |
| Quit | — | 是 | Destroy |
| SecondInstance / MainHidden | — | — | Keep（第二次启动不会创建 Widget） |

- **退出：** `quit()` 现在**第一步**就销毁 Widget，让它立即消失。然后才停止轮询、关闭 app-server、写 `app_stop`。
- **主窗口关闭：** 当 `close_to_tray = false` 时，关闭主窗口现在显式调用 `quit()`。以前依赖 "最后一个窗口关闭就退出"。有了 Widget 后，主窗口不再是最后一个窗口，进程会在没有主窗口的情况下继续运行。这是对主窗口行为的一处修改，见 §13。
- **健康日志：** 新增 `kind: "widget"` 事件：`created`、`updated`、`destroyed`（reason：disabled / closed / quit）、`create_skipped`、`create_failed`。每条附带当前 Widget 窗口数和 webview 窗口总数，供 72 小时 Widget soak 统计（§72–§76）。soak 报告新增了 "Desktop widget" 一节（`scripts/widget-summary.ts`，有单元测试）。

## 4. DTO：WidgetView

```
WidgetView {
  generatedAtMs, paused, globalSeverity, language, theme, mode, opacity(50–100), locked, updatedAtMs,
  providers: [ { id, displayName, shortLabel, fiveHourRemaining, weeklyRemaining,
                 state: live|stale|waiting|paused|error,
                 availability: allowed|blocked|unknown,
                 severity, updatedAtMs, note } ]
}
```

- 只包含**剩余**百分比，没有 used。
- **不包含：** 原始 snapshot、设置对象、路径、账号指纹（`json_shape_is_stable_for_the_page` 测试断言没有 `settings` 和 `snapshot` 字段）。
- `note` 由后端按应用语言生成。
- **可用性：** Codex 的 `availability` 只来自 `ordinaryUsageAllowed`；Claude 永远是 `unknown`。

## 5. 设置

| 设置 | 默认 | 说明 |
|---|---|---|
| 显示桌面小组件 | 关 | 关闭时窗口被**销毁**，不只是隐藏，释放 WebView2 renderer |
| 总在最前 | 开 | |
| 不透明度 | 85% | 50–100，松开滑块时才保存；超出范围的值在使用时被限制 |
| 布局 | 紧凑 | 紧凑 / 详细 |
| 记住位置 | 开 | |
| 锁定位置 | 关 | 在 Rust 中强制：锁定时 `widget_start_drag` 拒绝拖动 |

- **入口：** "设置" 页新增 "桌面小组件" 一节。Widget 右键菜单（打开 QuotaDock / 锁定位置 / 总在最前 / 详细模式 / 隐藏小组件）也走同一条后端路径修改设置，修改后通知主页面重新读取设置。
- **位置由后端负责：** 设置页保存整份设置时，保留后端存储的位置，所以持有旧副本的页面不会把 Widget 移回去。

**开发中发现并修复的问题：并发写设置。**
- **问题：** Widget 带来了三个可能同时写设置的来源：设置页、右键菜单、拖动后的位置保存。原来的 "读取 → 修改 → 写入" 没有加锁，一个写入方可能覆盖另一个的修改。
- **修复：** 新增 `modify_settings`，在一把写锁下完成读-改-写。`update_settings` 也使用这把锁，关闭提示的 "不再提示" 改用它；`save_settings_from_page` 则建立在它之上。
- **测试验证：** 8 个线程各做 5 次自增，加锁后结果正确（90）。去掉锁时测试失败（得到 55），证明测试能发现这个问题。

## 6. 显示规则

| 规格要求 | 实现 | 依据 |
|---|---|---|
| 只显示剩余 % | `remainingText`；没有任何 used 字段 | `widget_shows_remaining_not_used`、UI 测试 |
| 缺数据：— + 等待数据 | waiting 状态：数值 "—"，名称旁显示 "Waiting / 等待数据" | `widget_handles_missing_claude`、UI 测试 |
| 过期：上次的值 + 时间 | stale：数值变灰，显示 "47m ago / 47分钟前"（警告色） | `widget_handles_stale_claude`、UI 测试 |
| 暂停 | 标题显示 "Paused / 已暂停"，数值保留并变灰 | `widget_handles_paused_state`、UI 测试 |
| Codex 已达上限只来自权威字段 | 只在 `availability == blocked` 时显示 "已达到限制" | `availability_is_authoritative_for_codex_and_never_inferred_for_claude`；UI 测试：0% 但 allowed 时不显示 |
| Claude 可用性始终未知 | `availability: unknown` | 同上；UI 测试：Claude 0% 时也不显示 |
| 严重程度来自 Rust，用符号表示 | warning `!`、critical `!!`、error `×`（符号 + 颜色，不只靠颜色） | UI 测试；页面中没有阈值：数值很低时也不会自己变色 |
| 语言 / 主题跟随应用 | 视图带 `language` / `theme`，页面设置 `lang` 和 `data-theme` | `widget_language_follows_application`、`widget_theme_follows_application` |
| 错误 | 显示后端的提示（例如 "请先登录 Codex"）。紧凑模式下，没有任何数值的错误行把整行用于提示 | UI 测试、`[MOCK PREVIEW]` |

**布局。**
- **紧凑：** 标题行是 "QuotaDock" 加列标题（5h / Week）；每个 provider 一行：符号、名称、状态、两个数值。
- **详细：** 标题行显示数据时间或暂停；每个 provider 一行名称加两条剩余量条。

**`[MOCK PREVIEW]` 结果。** 第一轮渲染发现，紧凑模式下 "Limit reached"（需要 104 px，可用 89 px）和较长的错误提示被截断。修复后 22 个场景全部没有截断，内容高度与窗口高度一致（紧凑 101 / 79 px，详细 197 px）。

## 7. 交互

- **拖动：** 按下并移动 4 px 以上就开始系统拖动（`widget_start_drag`，锁定时被后端拒绝）。整个 Widget 都可以拖动，见 §13。
- **单击：** 按下后没有移动就松开。在标题或 provider 行上，打开主窗口；在背景上，不做任何事。
- **右键：** 由 Rust 构建原生菜单。所有菜单事件都会到达 app 的全局监听器（托盘）和窗口监听器；两边只处理自己的 id（`widget_*` 与托盘的 open / refresh / pause / quit 不重叠），所以每个 id 只被处理一次。菜单动作在主线程之外保存设置，因为写设置会占用数据库锁。
- **键盘：** 标题和 provider 行可以用 Tab 选中，按 Enter / 空格打开主窗口。
- 禁用了文字选择和 WebView2 默认右键菜单。

## 8. 安全边界

| 项目 | 状态 |
|---|---|
| capability | `capabilities/widget.json`：`windows: ["widget"]`，只有 `allow-get-widget-view`、`allow-open-main-window`、`allow-widget-start-drag`、`allow-widget-context-menu` |
| 双重检查 | 每个 Widget 命令都用 `only_widget` 检查调用窗口的 label |
| 主窗口 | `default.json` 仍是 18 个命令，只用于 `main`；不包含任何 Widget 命令 |
| 编译后检查 | 生成的 `gen/schemas/capabilities.json`：main 18 个权限，widget 4 个。Widget 页面的 JS 包只引用这 4 个命令名；主页面的 mock 数据不在 Widget 包中 |
| 不允许 | 写设置（由 Rust 的菜单完成）、provider 刷新、通知、文件系统、shell、SQL、RPC、Claude / Codex 命令、通用窗口 API、事件 API |
| CSP | 不变 |

## 9. P0：轮询节奏不变量

- **前台只看主窗口：** `provider_foreground(SurfaceStates)` = 主窗口可见 **且** 未最小化。Widget 是否可见或有焦点不参与判断。`sync_foreground` 已改用这个函数。
- **点击 Widget：** Windows 会激活 Widget，主窗口收到 `Focused(false)`，`sync_foreground` 按主窗口状态重新计算，结果不变。
- **Widget 的窗口事件：** 只交给 `widget.rs` 处理，永远不会把监控切换为前台。

| 测试 `[LOCAL TEST]` | 内容 |
|---|---|
| `widget_does_not_change_foreground_monitoring_state` | 遍历 Widget 可见 / 焦点的各种组合，结果只随主窗口变化 |
| `widget_does_not_change_codex_poll_cadence` | 暂停的 tokio 时钟；Widget 可见并有焦点，并被读取 20 次。轮询仍在 0 / 300 / 600 / 900 / 1200 s |
| `widget_does_not_change_claude_active_cadence` | Widget 可见时，Claude 仍按后台 15 min 判断；数据新鲜时不会额外查询 |
| `widget_does_not_trigger_provider_refresh` | 读取视图后：0 poll_runs、0 次 Claude CLI 调用、0 个 provider session |

真实环境下的节奏对比（Widget 关 / Widget 开 + 主窗口隐藏）按 §64 在 Core GO 之后进行。

## 10. 资源影响（待测量）

**按规定，Core GO 之前不测量。**

- **预期：** Widget 窗口与主窗口使用同一个 WebView2 环境，共享 browser / GPU / 网络进程，预计增加约 1 个 renderer 进程。
- **待记录项（§52 / §55 / §70）：**
  - Widget 关闭；
  - Widget 开启；
  - 主窗口隐藏 + Widget 显示（72 小时 soak 中的主要场景）；
  - 空闲 CPU；
  - 内存趋势和平台期。
- **参考值：** 当前 soak（没有 Widget）就是 "Widget 关闭" 的参考。截至 2026-10-11 23:31（soak 第 7 小时）：
  - WebView2 共 6 个进程，托盘状态中位数约 382 MB working set；
  - quotadock-app 约 41 MB；
  - CPU 平均 0.08%（托盘）。
- **包大小：**
  - Widget 页面 JS 7.6 kB，CSS 3.2 kB；
  - React 共享块 249 kB，与主页面共用。
- **降低开销：** 关闭 Widget 时销毁窗口，不保留隐藏的 renderer。

## 11. 测试

**Rust `crates/quotadock-core/tests/widget.rs`（25 个）：**
- §56 要求的 19 个名称全部存在：
  - `widget_is_disabled_by_default`、`enabling_widget_creates_one_window`、`single_instance_never_creates_second_widget`、`widget_uses_product_state_only`；
  - `widget_does_not_trigger_provider_refresh`、`widget_does_not_change_foreground_monitoring_state`、`widget_does_not_change_codex_poll_cadence`、`widget_does_not_change_claude_active_cadence`；
  - `widget_shows_remaining_not_used`、`widget_handles_missing_claude`、`widget_handles_stale_claude`、`widget_handles_paused_state`；
  - `widget_position_persists`、`offscreen_position_recovers`、`widget_settings_survive_restart`；
  - `widget_language_follows_application`、`widget_theme_follows_application`；
  - `closing_widget_does_not_exit_monitoring`、`quitting_app_closes_widget`。
- 另外 6 个：
  - `availability_is_authoritative_for_codex_and_never_inferred_for_claude`
  - `widget_window_has_no_taskbar_entry`
  - `widget_view_carries_display_settings`
  - `json_shape_is_stable_for_the_page`
  - `settings_page_save_keeps_the_widget_position`
  - `concurrent_settings_writers_never_undo_each_other`

**UI `ui/src/widget/present.test.ts`（10 个）：** 剩余量、等待、过期、暂停、错误、严重程度符号、"已达到限制" 只来自 availability、页面中没有阈值、标题、格式化。

**TS `tests/widgetSummary.test.ts`（4 个）：** soak 报告中的 Widget 摘要：显示时间、主窗口隐藏时的显示时间、重复创建检测、重启、报告窗口之前已打开的 Widget。另外用临时数据库和合成健康日志端到端运行了一次报告。

**合计：**
- Rust 176（Phase 3.6 结束时 151）；
- UI 34（24）；
- TS 参考实现 87（83）；
- `cargo clippy --workspace --all-targets` 零警告；
- UI 和根目录 `tsc --noEmit` 通过。

## 12. 限制与尚未验证

需要真实运行（Core GO 之后）`[PENDING REAL RUN]`：

1. Windows 11 上 WebView2 透明窗口的实际效果（圆角、边缘、无阴影）。
2. 创建时不抢焦点；点击 Widget 时会激活它（这是预期行为，见 backlog #24）。
3. `start_dragging` 在 WebView2 中的拖动手感；锁定时拒绝拖动。
4. 原生右键菜单的位置和关闭行为。
5. 置顶开 / 关；Win+D（显示桌面）时 Widget 的行为。
6. 真实 DPI / 多显示器下的位置恢复（本机只有自动化测试）。
7. WebView2 内存与 CPU（§10）。
8. §63 的 24 步手动验证、§64 的节奏对比、§65–§78 的 72 小时 Widget soak。

已知限制：
- 紧凑模式宽度固定为 224 逻辑像素，很长的后端提示会被截断（悬停提示和详细模式显示全文）。
- 崩溃后（没有 `destroyed` 事件），报告把 Widget 显示时间算到下一次 `app_start` 为止。

## 13. 与规格的差异 / 需要 GPT 知悉

1. **拖动区域：** 规格说 "背景是拖动区域"。现在整个 Widget 都可以拖动（移动 4 px 以上）；标题和 provider 行的单击仍然打开主窗口。规格中的两项都满足，只是拖动范围更大。
2. **主窗口关闭：** `close_to_tray = false` 时，关闭主窗口现在显式退出（`quit()`）。否则 Widget 打开时，进程会在没有主窗口的情况下继续运行。
3. **紧凑模式中没有数值的错误行：** 把整行用于提示，不显示 "— —"。等待数据的行仍然显示 "—"（按规格）。
4. **设置写锁：** 为了 Widget 的并发写入，在核心中加了 `modify_settings`。设置页保存时保留后端存储的 Widget 位置。
5. **soak 期间 baseline 的两次提交**（见 §0）：都不需要重新构建。仓库历史中仍有一个真实指纹前缀，推送前必须清理。
6. **退出顺序：** Widget 在退出流程的第一步就被销毁。
7. **历史页图表重做（2026-10-16，用户要求，不属于 Widget）：**
   - **为什么改：** 原图把 4 条曲线（两个 provider × 两个窗口）放在同一张图里，用实线、虚线、点线区分。重置标记也是橙色点线，容易和 Claude 曲线混淆；数据只覆盖范围的一小段时，大部分横轴是空的。
   - **新的布局：**
     - 每个窗口一个面板（5 小时、每周），每个面板里 Codex 蓝、Claude 橙两条实线。配色经过浅色与深色模式的校验。
     - 面板标题中的图例同时给出最新数值。
     - 数据少于范围时，横轴从最早的数据开始，并注明时间。
     - 两条曲线都没有数据的时段加灰底并标 "无数据"。
     - 重置标记改为上边缘的小圆点和浅色细线。
   - **交互：** 两个面板共用十字线和提示框，也可用键盘 ← → 查看；另有表格视图。
   - **新增 6 小时和 12 小时范围：** 用户要求，原因是正常工作时间很少连续用满 24 小时。核心的 `HistoryRange` 增加 `six_hours` / `twelve_hours`，和 24 小时一样显示每次读取，不降采样。历史页默认打开 12 小时，并在窗口的本地存储中记住上次选择的范围；存储不可用时回到默认值，不写设置也不写数据库。
   - **范围：** 两项都只改展示，不涉及轮询、Provider 或数据库结构。为了不影响 soak，只在本分支，随 RC 合并。

## 14. 下一步（PLAN A）

1. 继续当前 Core soak；期间只做只读检查。
   - **原计划：** 第 1 轮于 10-11 16:30:32 开始。
   - **中断：** 第 1 轮期间修复了 Claude 的 blocker（`79cebeb`，见 `SOAK_BASELINE_MANIFEST.md`）。
   - **重新计时：** 按 §80 重新计 7 天，第 2 轮从 2026-10-16 11:00:44 开始，**最早 2026-10-23 11:00:44 结束**。
2. 生成 `WINDOWS_V1_CORE_SOAK_FINDINGS.md`，给出 Core GO / NO-GO。
3. 如果 GO：
   - 把 `phase37-widget` 合并到 `windows-v1-rc-widget`，并跑完整回归；
   - 做 §63 手动验证和 §64 节奏验证；
   - 记录 §52 资源数据；
   - 进行 72 小时 Widget soak，生成 `PHASE37_WIDGET_SOAK_FINDINGS.md`。
4. 两个 GO 都通过之后，才开始 Windows V1 打包。
