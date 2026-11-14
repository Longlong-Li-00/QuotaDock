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

## 4. Observation of 1.1.0-rc.1

pid 51800 ran from 2026-11-05 09:41:42 until the user-approved upgrade quit at 2026-11-12 21:56:32
(7 days 12 h, far past the planned ~24 h). From `health.jsonl` and the database:

| Check | Result |
|---|---|
| Crash / unexpected stop | none; one `app_stop quit` line at the end |
| Resources (362 half-hourly samples) | app private 16.5 MB → 27.6 MB, handles 421 → 456, WebView x7 200 → 274 MB; no orphan Codex/Claude process (0/0 in the first and the last sample). The slow growth is real but small; it was not tracked sample by sample |
| Sleep / resume | OBSERVED: three suspend/resume pairs (11-10 12:26, 12:27, 14:43); no `monitoring_gap` or error line in the window. I did not check each poll after each resume |
| Database at the end | quick_check ok, 16 380 snapshots, latest id 16 380; backup `quotadock-pre-110rc2-20261112.sqlite3` (integrity ok, row counts equal) |
| Notifications in the window | 11-06 Claude weekly below 30 %, 11-09 Claude weekly below 10 %; all delivered |
| Claude observation | 1 781 active-CLI observations, `/usage` 1 781 ok / 1 error, auth state signed_in |

Still NOT observed: session end (`WM_ENDSESSION`), a logon autostart of 1.1, a real Claude sign-out,
the status-line bridge data flow, the quoted Run rewrite from Settings.

## 5. Upgrade 1.1.0-rc.1 → 1.1.0-rc.2 (2026-11-12)

Content: ring colour at 0 % (the whole ring takes the tone), display tones at four levels (≤50 % yellow,
≤30 % amber, ≤10 % red, 0 % empty). Reminders and severities keep 30 / 10. Commits `3b9c32c`, `273e5ed`,
then the version bump. User consent: "现在升级到 rc.2".

Installer `QuotaDock_1.1.0-rc.2_x64-setup.exe`, 4 420 762 B, SHA-256
`37EBA14EF0CA36E474BAFBD8C43725F09E4418D57FE168039182278C842E409D`.

| Check | Result |
|---|---|
| Backup before quitting | `.soak-backups\quotadock-pre-110rc2-20261112.sqlite3`, 16 380 snapshots, integrity ok, counts equal, main DB + WAL unchanged |
| Quit 1.1.0-rc.1 | `--quit` through a `.cmd` launched by explorer, stopped in ~1 s, one `app_stop quit` line (a direct `Start-Process explorer.exe "<exe> --quit"` did nothing: quote it in a `.cmd`) |
| Upgrade page | "覆盖安装并保留数据（推荐）" pre-selected |
| Directory page | **the install-folder field was EMPTY and Next disabled** (not so in the 1.1.0-rc.1 upgrade). Cause not found: `RestorePreviousInstallLocation` reads `HKCU\Software\QuotaDock\QuotaDock`, which held the right path. Going back and forward did not help. I set the field with `WM_SETTEXT` to the registry's own value `%LOCALAPPDATA%\Programs\QuotaDock`, after which Next and the free-space line appeared (backlog 60) |
| Finish page | "运行 QuotaDock" and "创建桌面快捷方式" ticked by default; `BM_CLICK` on 完成 did nothing, a space key message closed it and the app started |
| Install folder / data | unchanged path; same database, history kept |
| Installer backup | `backups\before-1.1.0-rc.2\` created, older `before-*` folders unchanged |
| State diff (pre vs post, `stage-e-state`) | only the version, pid, exe/uninstaller/bridge hashes and the new backup folder changed; Run value, StartupApproved, shortcuts and `settings.json` hash `B8842AAE8A728611` (no `statusLine`) are identical |
| App start | `app_start version 1.1.0-rc.2 launch normal`, `power_watch registered sessionEnd:true`, one process (pid 68572), widget restored (detailed, 55 %, locked, 224 x 171) |
| Widget with real data | Codex 75 % / 5-hour 82 %, Claude 69 %: all above 50 %, so no colour; the user's own logos show |
| First poll after start | Claude observation 22:00:59 (about 20 s after start); database quick_check ok |

Not observed in the installed build: the new yellow / amber / red / empty colours with real data (no
window is at or below 50 % right now). They were checked on mock sheets of the same UI build and by the
Rust and UI tests, not in the running app.

Note: the exe the installer put in the folder (SHA-256 `4ADBB03D…87B0`, written 21:55:38) differs from
`target\release\quotadock-app.exe` (`CF0E3FB4…E4B6`, 21:55:54) although both are 16 645 120 B; the
bundler rewrites the exe after embedding it. The bridge matches (`774CA1CF…FF9FA`). The installed hash
is the one to quote for this build.
