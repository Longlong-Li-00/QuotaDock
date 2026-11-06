# Windows V1 Soak Baseline Manifest

Recorded 2026-10-11 23:00 KST, while the 7-day dual-provider Core soak was running. Nothing listed here was stopped, rebuilt, replaced or modified to produce this manifest: the binaries were only read and hashed, and the production database was opened read-only by an unpackaged reader.

Paths are relative to the repository root unless noted; `%USERPROFILE%` stands for the user profile folder.

## Soak

**Current soak: run 2.** Decided 2026-10-16: after the Claude blocker fix (see "Interruptions and blocker fixes"), the full 7 days are counted again (PLAN A §80).

| Item | Value |
|---|---|
| Run 2 start | **2026-10-16 11:00:44 KST** (epoch 1792116044; `app_start` of pid 47140) |
| Run 2 earliest complete end | **2026-10-23 11:00:44 KST** |
| Run 2 binary | `target\release\quotadock-app.exe` `D5CCD2C9…` (commit `79cebeb`) |

The table below describes **run 1**. It ended with the Windows restart of 2026-10-14 13:31. Its data stays in the database, and it is reported separately.

| Item | Value |
|---|---|
| Soak | 7-day dual-provider Core soak (PHASE36_PRESOAK_FINDINGS.md §4) |
| Start | **2026-10-11 16:30:32 KST** (epoch 1791703832; first health-log line of the instance) |
| Earliest complete end | 2026-10-18 16:30:32 KST (superseded by run 2) |
| Process | `quotadock-app.exe`, pid 60332, parent `explorer.exe` (normal unpackaged launch, no arguments), running in the tray |
| Codex child | `codex app-server`, started 16:30:33, Codex 0.162.0-alpha.2 |
| Claude | Claude Code 2.1.278 (npm), subscription (claude.ai / firstParty / max); status-line bridge installed; active `/usage` refresh on |

## Binaries in use

| File | Size (bytes) | PE link time | SHA-256 |
|---|---|---|---|
| `target/release/quotadock-app.exe` (the running soak binary) | 16,147,456 | 2026-10-11 16:30:10 +09:00 | `F18DA65C932F0A87894C0135BF7C2A4371AAB63A0BA7BF4430F92360A06FC4A3` |
| `target/alt/release/quotadock-app.exe` (build output it was copied from) | 16,147,456 | 2026-10-11 16:30:10 +09:00 | identical |
| `target/release/quotadock-claude-bridge.exe` | 2,056,704 | 2026-10-11 16:29:32 +09:00 | `D8EA174C60710628B2D2757F1554FDC00C178C3779B0132BCDB0B38699E65EAA` |
| `%USERPROFILE%\.quotadock\bin\quotadock-claude-bridge.exe` (run by Claude Code) | 2,056,704 | 2026-10-11 16:29:32 +09:00 | identical |

Embedded UI (`ui/dist`, built 2026-10-11 16:29:08 and compiled into the exe at 16:30:10):

| File | SHA-256 (first 16 hex) |
|---|---|
| `ui/dist/index.html` | `3A74823635571830` |
| `ui/dist/assets/index-Dt5cOgqi.js` | `B8DBCB74C2331A44` |
| `ui/dist/assets/index-XknlIvEH.css` | `49FE9232A052B36C` |

## Runtime state at manifest time

| Item | Value |
|---|---|
| Database schema | v6 (`phase36_analysis_diagnostics`) |
| Reset detector | `reset-detector-v2` |
| Codex polls | all successful except one: 22:43:59, upstream `503 Service Unavailable` (`chatgpt.com/backend-api/wham/usage`, `error code: 1200`); the next poll at 22:45:00 succeeded. Not a QuotaDock fault |
| Provider (app-server) restarts | 0 |

## Toolchain

| Component | Version |
|---|---|
| Rust | rustc 1.99.0 (b940084d7 2026-09-28), cargo 1.99.0 |
| Tauri | tauri 2.12.1, tauri-build 2.7.1, tauri-plugin-notification 2.5.1, tauri-plugin-single-instance 2.5.2, tauri-plugin-autostart 2.7.0 |
| Other crates | rusqlite 0.40.2 (bundled SQLite), windows 0.62.2, webview2-com 0.39.1 (all pinned by `Cargo.lock`) |
| UI | React 19.3.0, Vite 8.3.3, TypeScript 7.0.2, @tauri-apps/api ^2.12.1 (pinned by `ui/package-lock.json`) |
| Node | v24.14.0 (UI build, TS reference tests, soak report) |
| OS | Windows 11 Home China 10.0.26200 |

## Source / binary correspondence

The repository had no version control when the binary was built, so this cannot be proven cryptographically.

**Assessment: high confidence. Verified by rebuild on 2026-10-16.**

Verification:
- **Rebuild:** the tagged source (at `0d275c8`, whose post-tag commits change no compiled code) was rebuilt with the same toolchain, `Cargo.lock` and `ui/dist`. The result has the soak binary's exact size, 16,147,456 bytes.
- **Byte comparison:** only 28 bytes differ: the PE header timestamp, three debug-directory timestamps and the 16-byte PDB GUID.

Evidence:

1. The build ran after the last edit to any source that is compiled into the app.
   - Newest such files: `crates/quotadock-core/src/{analytics/config.rs, analytics/forecast.rs, product/claude_view.rs}` at 16:26:37 and `src-tauri/src/main.rs` at 16:22:50.
   - The newest test file is 16:27:44.
   - The UI bundle was built at 16:29:08, and the exe was linked at 16:30:10.
   - No compiled source file has a modification time after the link time.
2. The running exe is byte-identical (SHA-256) to the build output in `target/alt/release`.
3. Between the build and this manifest, only documentation was written (`PHASE36_PRESOAK_FINDINGS.md`, `POST_V1_BACKLOG.md`) plus this manifest; no build was run.
4. All test suites passed against this source tree immediately before the build (Rust 151, UI 24, TS 83, clippy clean).

**Differences between the baseline commit and the source of the binary** (sanitization before the first commit, as required by the Git hygiene rules; neither difference is executed by the running app):

| File | Change | Effect on the soak binary |
|---|---|---|
| `ui/src/api/mock.ts` | The mock dashboard's short account fingerprint (copied from real data) was replaced by a fake value | None: mock data is only used when the UI runs outside Tauri (browser preview) |
| `CODEX_PROVIDER_FINDINGS.md` | One user-profile path replaced by `%USERPROFILE%` | Documentation only |

Not in version control by design: build output (`target/`, `ui/dist/`, `src-tauri/gen/`), `node_modules/`, all QuotaDock runtime data (`%LOCALAPPDATA%\QuotaDock`: database, WAL/SHM, health log, diagnostics exports, Claude settings backups) and the bridge's runtime files under `%USERPROFILE%\.quotadock`.

## Git

| Item | Value |
|---|---|
| Baseline branch | `v1-soak-baseline` |
| Baseline tag | `windows-v1-soak-baseline-20261011` |
| Widget development branch | `phase37-widget` (from the tag; separate worktree `.worktrees/phase37-widget`, never deployed to this soak) |

Blocker fixes during the soak are made on `v1-soak-baseline`, tested, rebuilt and recorded here (interruption time, old/new SHA-256, bug, fix), then cherry-picked to `phase37-widget`. Never the other way round.

## Interruptions and blocker fixes

### 2026-10-16 — Claude active source latched by a /usage text without usage lines (V1 blocker)

**Interruptions**

| What | From | Evidence |
|---|---|---|
| Claude unmonitored while the app ran | 2026-10-13 14:06:08 (last stored Claude data) | The 14:21:09 `/usage` query returned an answer without usage lines. The run was recorded as `unrecognized` and the health log has `claude_active needs_verification`. No later Claude query, no notification. |
| Network outage at that time | 2026-10-13 11:54 – 14:33 | 11 Codex polls failed (`error sending request` or 30 s timeout). The Codex provider-error notification was sent at 14:03, and Codex recovered on its own. |
| QuotaDock not running | 2026-10-14 13:31 | The user restarted Windows from the Start menu at 13:31. Later the same day: another restart at 19:33, a Windows Update restart at 19:36, a shutdown at 19:37, boot at 20:00. The last Codex poll was at 13:27:31. There is no `app_stop` record (the exit at Windows end of session does not record one), and QuotaDock was not set to start with Windows. |

**Bug.** `parse_usage_text` reported a `/usage` text that has no usage line as `Unrecognized`, which latches `needs_verification` until QuotaDock restarts. The JSON envelope of that answer had already proved there was no model use: local command `usage`, zero turns, zero tokens. So a temporary failure of Claude Code's local `/usage` (here during a network outage) stopped Claude monitoring silently for the rest of the run.

**Fix (minimal).**
- **New error kind:** `UsageQueryError::NoUsageLines`. It is handled like other temporary errors: back-off of 15 / 30 / 60 min, no latch, automatic recovery on the next good answer.
- **What is stored:** only a description of the text (length, line count, which of a fixed list of words it mentions), never the text itself. The run status is `no_usage_lines`.
- **Still latched:** model usage in the envelope, a malformed envelope, and usage lines in an unknown shape (a format change).

**Tests.**
- **Regression test:** `usage_text_without_usage_lines_backs_off_and_recovers`. It fails on the old code and passes with the fix.
- **New test:** `usage_lines_in_an_unknown_shape_need_verification`.
- **Results:** Rust 152, UI 24, TS 83 all pass; clippy reports no warnings; the typechecks pass.

**Binaries**

| | SHA-256 | Size |
|---|---|---|
| Old (soak since 2026-10-11) | `F18DA65C932F0A87894C0135BF7C2A4371AAB63A0BA7BF4430F92360A06FC4A3` | 16,147,456 bytes |
| New (built 2026-10-16 10:51) | `D5CCD2C9FB003B8E7D51939D30556D703FEBB1D038C1969248E1D9A41CEFFDBB` | 16,088,576 bytes |

The new binary was built from the commit that adds this section. It uses the same toolchain, `Cargo.lock` and `ui/dist` (hashes unchanged). A second build of it differed only in timestamps (24 bytes). The bridge is unchanged: it was not rebuilt, and the installed copy is still `D8EA174C…`.

**Soak resumption.**
- **Install:** at 2026-10-16 11:00 the new binary was copied to `target\release\quotadock-app.exe` (verified `D5CCD2C9…`). The old one is kept as `target\release\quotadock-app.soak1-F18DA65C.exe`.
- **Start:** launched through Explorer; app start at 11:00:44, pid 47140, normal launch.
- **First runs:**
  - Codex startup poll at 11:00:46: success.
  - Claude `/usage` at 11:00:44: success, data stored at 11:00:55.
  - QuotaDock's own Codex app-server is running.
- **Soak timing (decided 2026-10-16):** under PLAN A §80, a new full 7-day soak, counted from 2026-10-16 11:00:44. The earliest complete end is 2026-10-23 11:00:44.

## Commits after the baseline tag

Unless stated otherwise, the soak was not interrupted and the exe was not rebuilt for these.

| Date (KST) | Change | Effect on the soak binary |
|---|---|---|
| 2026-10-11 | Hygiene: `ui/src/api/mock.ts` still held a second short account fingerprint copied from real data (the mock Claude card). It was replaced by a fake value. The tagged commit still contains the old value in history, so the repository must not be published as is. Before any push, rewrite or squash the history. | None: mock data only (browser preview); no rebuild |
| 2026-10-11 | Soak report classification (`scripts/soak-report.mjs`, read-only tool). A gap that started while the main window was shown was judged against the 60 s / 5 min foreground cadence even when the window was hidden 10–60 s later and polling correctly returned to the tray cadence. The first 7 h of the soak showed 3 such Codex gaps as "over threshold, unexplained", plus one Claude gap as "unknown". Each was exactly 300 s (Codex) or 15 min (Claude), and the in-app watchdog, which uses the controller's real delay, opened 0 MONITORING_GAP. The foreground cadence now applies only if the window stayed shown for the whole gap. Re-run result: 0 over threshold, 0 unexplained. | None: the report runs outside the app; no rebuild, no restart |

## After the Core soak (Phase 3.8 hotfix)

- **End of run 2:** Windows restarted on 2026-10-26: restart requested at 09:15:55 (System event 1074), clean shutdown 09:16:10 (6006), boot 09:16:48 (6005). The last Codex snapshot is #10609 at 09:15:38. QuotaDock has no autostart and has not run since. The run-2 binary `D5CCD2C9…` is unchanged at `target\release\quotadock-app.exe`.
- **Stage A commits on this branch:** `6be0d1c` (reset-detector-v3), `fb8c32a` (Claude sign-out notice), `4204f0e` and `c38a9d0` (fixes from an independent review), and the commits with `PHASE38_CORE_HOTFIX_FINDINGS.md`. None of them ran in either soak run.
- **History check:** the v3 re-analysis ran on copies of the production database made through a read-only connection (`PHASE38_CORE_HOTFIX_FINDINGS.md` §4). The production database was not written.
- **Tag:** `windows-v1-soak-baseline-20261011` is unchanged.
