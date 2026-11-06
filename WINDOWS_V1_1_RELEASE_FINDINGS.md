# QuotaDock 1.1.0-rc.1 — upgrade acceptance and observation

Branch `v1.1-ring-widget`. Scope (user decision 2026-11-04): ring widget, small backlog fixes, dynamic
tray icon (two bars). Not in scope: 13→3 reminder switches, Events page, extra forecasts.

## 1. Before the upgrade

- Isolated real-window pass (2026-11-04 night, fake Codex/Claude data): ring heights 187/171/94 as
  computed; Claude 5-hour line was cut → `~HH:MM` (ec0dd3e), fits after the rebuild; tray bars seen
  at real size; Codex stop 462 ms (was ~3 s); `display.json` follows the 5-hour switch.
- Mistake during that pass: the isolated instance is not isolated for `~/.claude` / `~/.quotadock`
  (backlog 57). Enabling the bridge there changed the real `~/.claude/settings.json`; restored the
  same night with the app's own "停用并恢复" (file hash B8842AAE8A728611 again, no `statusLine`).

## 2. Upgrade 1.0.0 → 1.1.0-rc.1 on the production install (2026-11-05)

Installer `QuotaDock_1.1.0-rc.1_x64-setup.exe`, SHA-256
`B97DA86C315A470F5BC1B466D6A6E72141494BE5FA90BAD41B7B56AD4C6EF346`. User consent: "现在升级，开始升级验收".

| Check | Result |
|---|---|
| Manual backup before quitting (online backup, read-only source) | `.soak-backups\quotadock-pre-110rc1-20261105.sqlite3`, integrity ok, row counts equal (13 681 snapshots), main DB + WAL unchanged |
| Quit 1.0.0 (`--quit`) | stopped in 3.9 s; one `app_stop quit` line |
| Upgrade page | "已安装 … 可以直接覆盖安装，你的数据会保留", "覆盖安装并保留数据（推荐）" pre-selected |
| Install folder | unchanged, `%LOCALAPPDATA%\Programs\QuotaDock` |
| Installer backup | `backups\before-1.1.0-rc.1\` created; all older `before-*` folders kept unchanged |
| Database after start | same file, history kept (counts not reset) |
| Uninstall entry | `QuotaDock 1.1.0-rc.1` |
| Autostart | was on (unquoted 1.0 value); now `"…\quotadock-app.exe" --autostart` (written by the installer, which only rewrites an existing value); StartupApproved unchanged (enabled) |
| Shortcuts | Start menu and desktop, AUMID `com.quotadock.app` |
| App start | `app_start version 1.1.0-rc.1 launch normal`, `power_watch registered sessionEnd:true`, one process, one widget |
| Widget settings kept | detailed, not on top, opacity 55, locked, position restored |
| Ring widget with real data | Codex: outer weekly 91 %, inner 5-hour 100 %; Claude (5-hour hidden): single thicker ring 62 %; height 171 as computed |
| Tray icon with real data | two grey bars, left (Codex 91 %) taller than right (Claude 62 %) |
| Claude status line | bridge absent, `settings.json` hash B8842AAE8A728611, no `statusLine` (kept off) |

## 3. Not verified / not observed (as of the upgrade)

- App-side quoted Run rewrite (Settings → autostart off/on): NOT VERIFIED in a real run.
- Real session end (`WM_ENDSESSION` → `session_end` line): NOT OBSERVED.
- Real logon autostart of 1.1: NOT OBSERVED (next reboot).
- Sleep/resume, real Claude sign-out: NOT OBSERVED.
- Low/critical tray colours with real data: NOT OBSERVED (only with fake data).
- User's own logo files (`%LOCALAPPDATA%\QuotaDock\icons\`): VERIFIED 2026-11-06 with PNG only.
  The user's first files (1254 px, 617/687 KB) were over the 256 KB limit and ignored, as designed
  (built-in icons stayed, no error shown). Originals moved to `icons\original\`, 128 px copies
  (8/12 KB) written; both appeared in the running widget without a restart. SVG not tried. The
  silent size limit is easy to miss: the user is not told why a file is ignored (candidate backlog).
- Status-line bridge data flow with a real Claude Code message: NOT VERIFIED.

## 4. Observation

~24 h from 2026-11-05 09:41:42 (pid 51800). Results to be added here.
