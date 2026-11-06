// QuotaDock soak report (Windows V1, Phase 3.6 §24–§33). Strictly read-only: the database is
// opened with readOnly (WAL lets it run while QuotaDock runs), the health log is only read, and
// the System event log is only queried. Prints counts and times only: no prompts, account values
// or user file paths.
//
//   node scripts/soak-report.mjs [--days 7] [--db <path>] [--no-eventlog]
//   npm run soak:report -- --days 1
import { DatabaseSync } from "node:sqlite";
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { execFileSync } from "node:child_process";
import { summarizeWidget } from "./widget-summary.ts";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(name);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const flag = (name) => process.argv.includes(name);
const days = Number(arg("--days", "7"));
const dbPath = arg("--db", join(process.env.LOCALAPPDATA ?? "", "QuotaDock", "quotadock.sqlite3"));
const dataDir = dirname(dbPath);
const db = new DatabaseSync(dbPath, { readOnly: true });
const now = Date.now();
const since = now - days * 86_400_000;
const q = (sql, ...p) => db.prepare(sql).all(...p);
const one = (sql, ...p) => db.prepare(sql).get(...p);
const has = (t) => one("SELECT COUNT(*) n FROM sqlite_master WHERE type='table' AND name=?", t).n > 0;
const ts = (ms) => (ms ? new Date(ms).toLocaleString("sv-SE").slice(0, 19) : "-");
const mins = (ms) => `${(ms / 60_000).toFixed(1)} min`;
const pct = (a, b) => (b ? `${((100 * a) / b).toFixed(1)}%` : "-");
const mb = (b) => (b == null ? "-" : `${(b / 1_048_576).toFixed(1)} MB`);
const out = [];
const p = (s = "") => out.push(s);
const h1 = (s) => p(`\n## ${s}`);
const table = (rows) => rows.forEach((r) => p("  " + Object.values(r).join("  ")));
const count = (arr, f) => arr.reduce((m, x) => ((m[f(x)] = (m[f(x)] ?? 0) + 1), m), {});

// ---------------------------------------------------------------------------------- health log
const health = [];
for (const f of ["health.1.jsonl", "health.jsonl"]) {
  const path = join(dataDir, "logs", f);
  if (!existsSync(path)) continue;
  for (const line of readFileSync(path, "utf8").split("\n")) {
    if (!line.trim()) continue;
    try {
      const v = JSON.parse(line);
      if (v.t >= since - 86_400_000) health.push(v);
    } catch {
      /* torn last line while the app writes: ignore */
    }
  }
}
health.sort((a, b) => a.t - b.t);
const hk = (kind) => health.filter((h) => h.kind === kind && h.t >= since);
/** Value of a state recorded by `kind` lines ({value}) at time t (last line before t). */
const stateAt = (kind, t, fallback) => {
  let v = fallback;
  for (const h of health) {
    if (h.t > t) break;
    if (h.kind === kind) v = h.value;
  }
  return v;
};

// ------------------------------------------------------------------------------ system event log
const machine = [];
if (!flag("--no-eventlog") && process.platform === "win32") {
  try {
    const ps = `$e = Get-WinEvent -FilterHashtable @{LogName='System'; StartTime=(Get-Date).AddDays(-${days}); Id=12,13,41,42,107,6008} -ErrorAction SilentlyContinue | Where-Object { $_.ProviderName -match 'Kernel-General|Kernel-Power|EventLog' } | Select-Object @{n='t';e={[DateTimeOffset]::new($_.TimeCreated).ToUnixTimeMilliseconds()}}, Id, ProviderName; if ($e) { $e | ConvertTo-Json -Compress }`;
    const raw = execFileSync("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", ps], { encoding: "utf8", timeout: 60_000 }).trim();
    const list = raw ? JSON.parse(raw) : [];
    for (const e of Array.isArray(list) ? list : [list]) {
      const kind =
        e.Id === 12 ? "boot" : e.Id === 13 ? "shutdown" : e.Id === 41 ? "unexpected_restart" : e.Id === 6008 ? "unexpected_shutdown" : e.Id === 42 ? "sleep" : "wake";
      machine.push({ t: e.t, kind });
    }
    machine.sort((a, b) => a.t - b.t);
  } catch {
    /* event log unavailable: classification falls back to the health log */
  }
}
const suspendedIn = (a, b) =>
  machine.some((m) => m.t >= a && m.t <= b && ["sleep", "shutdown", "boot", "unexpected_restart", "unexpected_shutdown"].includes(m.kind)) ||
  health.some((h) => h.kind === "power" && h.t >= a && h.t <= b);
const appStartIn = (a, b) => health.some((h) => h.kind === "app_start" && h.t > a && h.t <= b);
const pausedAt = (t) => stateAt("paused", t, false);
const fgAt = (t) => stateAt("foreground", t, false);
/** Foreground for the whole interval: shown at `a` and not hidden before `b`. A window hidden during
 *  the gap puts polling back on the tray cadence, which is then the expected one. */
const fgThroughout = (a, b) => fgAt(a) && !health.some((h) => h.kind === "foreground" && h.value === false && h.t > a && h.t <= b);

/** Why a gap happened [QUOTADOCK HEURISTIC]. */
function classify(a, b, expectedMs, failedNearby, endsWithStartup = false) {
  if (pausedAt(a + 1)) return "app paused";
  if (suspendedIn(a, b)) return "machine sleep/shutdown";
  if (appStartIn(a, b) || endsWithStartup) return "app not running (restarted)";
  if (failedNearby) return "provider error/backoff";
  if (b - a <= expectedMs * 1.5) return "expected";
  return "unknown";
}

p(`# QuotaDock soak report`);
p(`last ${days} day(s): ${ts(since)} → ${ts(now)}`);
p(`database: ${dbPath} (schema v${one("SELECT MAX(version) v FROM schema_migrations").v})`);

// ------------------------------------------------------------------------------- integrity & size
h1("Database integrity and size");
const integrity = q("PRAGMA integrity_check").map((r) => Object.values(r)[0]);
p(`integrity_check: ${integrity.join(", ")}`);
for (const f of ["", "-wal", "-shm"]) {
  const path = dbPath + f;
  if (existsSync(path)) p(`  ${("quotadock.sqlite3" + f).padEnd(24)} ${mb(statSync(path).size)}`);
}
const counts = (t, where = "") => (has(t) ? one(`SELECT COUNT(*) n FROM ${t} ${where}`).n : "-");
p(
  `rows: snapshots ${counts("snapshots")}, quota_events ${counts("quota_events")}, poll_runs ${counts("poll_runs")}, provider_snapshots ${counts("provider_snapshots")}, provider_events ${counts("provider_events")}, notification_log ${counts("notification_log")}, analysis_diagnostics ${counts("analysis_diagnostics")}`,
);

// ------------------------------------------------------------------------------ run / uptime
h1("Run duration, app uptime, machine availability");
const starts = hk("app_start");
const stops = hk("app_stop");
p(`app starts: ${starts.length} (${Object.entries(count(starts, (s) => `${s.launch}${s.relaunched ? "+relaunched" : ""}`)).map(([k, v]) => `${k} ${v}`).join(", ") || "-"}); clean stops: ${stops.length}`);
const unclean = starts.filter((s, i) => i > 0 && !health.some((h) => h.kind === "app_stop" && h.t < s.t && h.t > starts[i - 1].t));
p(`starts without a preceding clean stop (crash, forced end or machine shutdown): ${unclean.length}`);
if (machine.length) p(`machine: ${Object.entries(count(machine, (m) => m.kind)).map(([k, v]) => `${k} ${v}`).join(", ")}`);
p(`power events seen by QuotaDock: ${hk("power").length}; power watch registered: ${health.filter((h) => h.kind === "power_watch").map((h) => h.registered).join(",") || "-"}`);
const polls = has("poll_runs") ? q("SELECT id, started_at_ms s, finished_at_ms f, poll_trigger tr, status st, error_message e FROM poll_runs WHERE started_at_ms >= ? ORDER BY started_at_ms", since) : [];
if (polls.length) {
  const span = polls[polls.length - 1].s - polls[0].s;
  // Monitored time: sum of poll intervals that are not "app down" (> 20 min without a poll).
  let covered = 0;
  for (let i = 1; i < polls.length; i++) covered += Math.min(polls[i].s - polls[i - 1].s, 20 * 60_000);
  p(`first poll ${ts(polls[0].s)}, last poll ${ts(polls[polls.length - 1].s)} (${(span / 3_600_000).toFixed(1)} h); monitored (gaps ≤ 20 min) ${(covered / 3_600_000).toFixed(1)} h = ${pct(covered, span)}`);
}

// ------------------------------------------------------------------------------------- Codex
h1("Codex reliability");
const ok = polls.filter((r) => r.st === "success").length;
p(`polls: ${polls.length}, success ${ok}, failed ${polls.length - ok}, success rate ${pct(ok, polls.length)}`);
p(`by trigger: ${Object.entries(count(polls, (r) => r.tr)).map(([k, v]) => `${k} ${v}`).join(", ")}`);
const failures = polls.filter((r) => r.st !== "success");
if (failures.length) {
  p("recent failures:");
  failures.slice(-5).forEach((e) => p(`  ${ts(e.s)}  ${e.st}  ${(e.e ?? "").slice(0, 100)}`));
}
const gaps = polls.slice(1).map((r, i) => {
  const a = polls[i].s;
  const fg = fgThroughout(a, r.s);
  const expected = fg ? 60_000 : 300_000;
  const failedNearby = polls[i].st !== "success";
  return { a, b: r.s, d: r.s - a, expected, fg, paused: pausedAt(a + 1), why: classify(a, r.s, expected, failedNearby, r.tr === "startup") };
});
p("Top 10 longest Codex poll gaps (start → end, duration, expected cadence, window, cause):");
[...gaps].sort((x, y) => y.d - x.d).slice(0, 10).forEach((g) => p(`  ${ts(g.a)} → ${ts(g.b)}  ${mins(g.d).padStart(10)}  expected ${g.expected / 1000}s  ${g.fg ? "window shown" : "tray"}  ${g.paused ? "paused" : ""}  ${g.why}`));
const longGaps = gaps.filter((g) => g.d > (g.fg ? 3 : 2) * g.expected);
p(`gaps over the MONITORING_GAP threshold: ${longGaps.length} (${Object.entries(count(longGaps, (g) => g.why)).map(([k, v]) => `${k} ${v}`).join(", ") || "none"}); unexplained: ${longGaps.filter((g) => g.why === "unknown").length}`);
if (has("provider_sessions")) {
  const ses = q("SELECT started_at_ms t, reason, cli_version v FROM provider_sessions WHERE started_at_ms >= ? ORDER BY id", since);
  p(`provider (app-server) launches: ${ses.length} (${Object.entries(count(ses, (s) => s.reason)).map(([k, v]) => `${k} ${v}`).join(", ")}); unexpected exits (after_exit): ${ses.filter((s) => s.reason === "after_exit").length}`);
  const versions = [...new Set(ses.map((s) => s.v))];
  p(`Codex versions: ${versions.join(" → ") || "-"}${versions.length > 1 ? "  (VERSION CHANGE)" : ""}`);
}
const classifier = one("SELECT classifier_version v FROM engine_state ORDER BY updated_at_ms DESC LIMIT 1")?.v ?? "reset-detector-v1";
const ev = q("SELECT id, event_type t, COALESCE(window_type,'account') w, visibility vis, confidence c, observed_at_ms at, previous_snapshot_id ps, current_snapshot_id cs, superseded_by_event_key sup FROM quota_events WHERE classifier_version = ? AND observed_at_ms >= ? ORDER BY id", classifier, since);
p(`events (${classifier}):`);
table(Object.entries(count(ev, (e) => `${e.t}|${e.w}|${e.vis}`)).map(([k, n]) => ({ k: k.replaceAll("|", " / ").padEnd(58), n })));
const nOf = (t, w) => ev.filter((e) => e.t === t && (!w || e.w === w) && !e.sup).length;
const reanchors = has("analysis_diagnostics") ? q("SELECT created_at_ms t, snapshot_id s, data_json d FROM analysis_diagnostics WHERE kind='reanchor' AND created_at_ms >= ? ORDER BY id", since) : [];
p(`5h: natural rollovers ${nOf("natural_rollover_observed", "five_hour")}, sliding anchors ${nOf("sliding_anchor_detected", "five_hour")}, idle→active re-anchors ${reanchors.length}, transient resetsAt ${nOf("transient_backend_state", "five_hour")}`);
p(`weekly: natural rollovers ${nOf("natural_rollover_observed", "weekly")}, schedule changes ${nOf("window_schedule_changed", "weekly")} (+${ev.filter((e) => e.t === "window_schedule_changed" && e.w === "weekly" && e.sup).length} reverted), sliding anchors ${nOf("sliding_anchor_detected", "weekly")}, transient ${nOf("transient_backend_state", "weekly")}`);
p(`ResetCreditRedeemed ${nOf("reset_credit_redeemed")}, UnscheduledQuotaRefresh ${nOf("unscheduled_quota_refresh")}`);
const snaps = q("SELECT id, captured_at_ms c, ordinary_usage_allowed a, five_hour_present fp, five_hour_reset_at_ms fr, five_hour_duration_mins fd, five_hour_used_percent fu, weekly_present wp, weekly_reset_at_ms wr, weekly_duration_mins wd, weekly_used_percent wu FROM snapshots WHERE captured_at_ms >= ? ORDER BY id", since);
const allowedChanges = snaps.slice(1).filter((s, i) => s.a !== snaps[i].a);
p(`ordinaryUsageAllowed transitions: ${allowedChanges.length}${allowedChanges.map((s) => `  ${ts(s.c)} → ${s.a}`).join("")}`);
p(`missing windows: 5h ${snaps.filter((s) => !s.fp).length}, weekly ${snaps.filter((s) => !s.wp).length} of ${snaps.length} snapshots`);

// Before / after of every quota boundary (§36).
const snapById = new Map(q("SELECT id, captured_at_ms c, ordinary_usage_allowed a, five_hour_used_percent fu, five_hour_reset_at_ms fr, weekly_used_percent wu, weekly_reset_at_ms wr FROM snapshots WHERE captured_at_ms >= ?", since - 86_400_000).map((s) => [s.id, s]));
const boundaries = ev.filter((e) => ["natural_rollover_observed", "unscheduled_quota_refresh", "reset_credit_redeemed"].includes(e.t) && !e.sup);
if (boundaries.length) {
  p("quota boundaries (before → first snapshot after):");
  for (const e of boundaries) {
    const [b, a] = [snapById.get(e.ps), snapById.get(e.cs)];
    const pick = (s) => (s ? (e.w === "weekly" ? `${s.wu}% reset ${ts(s.wr)}` : `${s.fu}% reset ${ts(s.fr)}`) : "?");
    p(`  ${ts(e.at)} ${e.w} ${e.t} (${e.c}): ${pick(b)} → ${pick(a)}${a ? ` allowed=${a.a}` : ""}`);
  }
}

// resetsAt stability (§28): consecutive deltas, idle sliding windows reported apart.
h1("Codex resetsAt stability");
for (const [w, rk, dk] of [["five_hour", "fr", "fd"], ["weekly", "wr", "wd"]]) {
  const buckets = { "0 s": 0, "±1 s": 0, "±2 s": 0, "3–10 s": 0, ">10 s": 0, "sliding (idle)": 0, "quota boundary": 0 };
  const boundaryIds = new Set(ev.filter((e) => e.w === w && ["natural_rollover_observed", "unscheduled_quota_refresh", "reset_credit_redeemed", "window_schedule_changed", "sliding_anchor_detected"].includes(e.t)).flatMap((e) => [e.cs]));
  const isSliding = (s) => s[rk] && s[dk] && Math.abs(s[rk] - (s.c + s[dk] * 60_000)) < 5_000;
  for (let i = 1; i < snaps.length; i++) {
    const [x, y] = [snaps[i - 1], snaps[i]];
    if (!x[rk] || !y[rk]) continue;
    if (isSliding(x) && isSliding(y)) {
      buckets["sliding (idle)"]++;
      continue;
    }
    const d = Math.abs(y[rk] - x[rk]) / 1000;
    if (d > 10 && (boundaryIds.has(y.id) || boundaryIds.has(snaps[i + 1]?.id) || boundaryIds.has(snaps[i + 2]?.id))) {
      buckets["quota boundary"]++;
      continue;
    }
    buckets[d === 0 ? "0 s" : d <= 1 ? "±1 s" : d <= 2 ? "±2 s" : d <= 10 ? "3–10 s" : ">10 s"]++;
  }
  p(`  ${w.padEnd(9)} ${Object.entries(buckets).map(([k, v]) => `${k}: ${v}`).join("  ")}`);
}

// ------------------------------------------------------------------------------------- Claude
h1("Claude reliability");
if (has("provider_snapshots")) {
  for (const e of health.filter((h) => h.kind === "claude_env")) p(`  ${ts(e.t)} Claude Code ${e.version} (${e.installKind}) billing=${e.billing} auth=${e.authMethod}/${e.apiProvider} plan=${e.subscriptionType} loggedIn=${e.loggedIn}`);
  const cs = q("SELECT id, source, captured_at_ms c, source_version v FROM provider_snapshots WHERE provider='claude' AND captured_at_ms >= ? ORDER BY captured_at_ms", since);
  const bySource = count(cs, (s) => s.source);
  p(`snapshots: ${cs.length} — statusLine ${bySource.statusline ?? 0} (${pct(bySource.statusline ?? 0, cs.length)}), active_cli ${bySource.active_cli ?? 0} (${pct(bySource.active_cli ?? 0, cs.length)})`);
  const versions = [...new Set(cs.map((s) => s.v))];
  p(`Claude Code versions in snapshots: ${versions.join(" → ") || "-"}${versions.length > 1 ? "  (VERSION CHANGE)" : ""}`);
  const runs = q("SELECT started_at_ms s, finished_at_ms f, status st, error_message e FROM provider_source_runs WHERE provider='claude' AND started_at_ms >= ? ORDER BY started_at_ms", since);
  const byStatus = count(runs, (r) => r.st);
  p(`active /usage queries: ${runs.length} — ${Object.entries(byStatus).map(([k, v]) => `${k} ${v}`).join(", ") || "-"}`);
  p(`  429 (rate_limited): ${byStatus.rate_limited ?? 0}; errors: ${byStatus.error ?? 0}; needs_verification (inference_detected/unrecognized): ${(byStatus.inference_detected ?? 0) + (byStatus.unrecognized ?? 0)}`);
  p(`  active source suspensions: ${hk("claude_active").filter((h) => h.event === "suspended_after_rate_limits").length}; needs_verification events: ${hk("claude_active").filter((h) => h.event === "needs_verification").length}`);
  const dur = runs.map((r) => r.f - r.s).sort((a, b) => a - b);
  if (dur.length) p(`  duration: median ${dur[Math.floor(dur.length / 2)]} ms, p95 ${dur[Math.floor(dur.length * 0.95)]} ms, max ${dur[dur.length - 1]} ms`);
  const runGaps = runs.slice(1).map((r, i) => r.s - runs[i].s);
  if (runGaps.length) p(`  longest gap between active queries: ${mins(Math.max(...runGaps))}`);
  const cg = cs.slice(1).map((r, i) => {
    const a = cs[i].c;
    const fg = fgThroughout(a, r.c);
    const expected = fg ? 5 * 60_000 : 15 * 60_000;
    return { a, b: r.c, d: r.c - a, expected, fg, why: classify(a, r.c, expected, false) };
  });
  p("Top 10 longest Claude update gaps:");
  [...cg].sort((x, y) => y.d - x.d).slice(0, 10).forEach((g) => p(`  ${ts(g.a)} → ${ts(g.b)}  ${mins(g.d).padStart(10)}  expected ${g.expected / 60_000} min  ${g.fg ? "window shown" : "tray"}  ${g.why}`));
  const stale = cg.filter((g) => g.d > 20 * 60_000);
  p(`stale periods (> 20 min without an update while active refresh runs): ${stale.length}, total ${mins(stale.reduce((s, g) => s + g.d - 20 * 60_000, 0))} beyond the live window; unexplained: ${stale.filter((g) => g.why === "unknown").length}`);
  p("window instances:");
  table(q("SELECT window_key, status, COUNT(*) n FROM provider_window_instances WHERE provider='claude' AND opened_at_ms >= ? GROUP BY 1, 2 ORDER BY 1, 2", since));
  p(`extra bucket observations: ${one("SELECT COUNT(*) n FROM provider_windows w JOIN provider_snapshots s ON s.id = w.snapshot_id WHERE s.provider='claude' AND w.kind='extra' AND s.captured_at_ms >= ?", since).n}`);
  const cev = q("SELECT event_type t, COALESCE(window_key,'account') w, visibility v, observed_at_ms at, evidence_json e FROM provider_events WHERE provider='claude' AND observed_at_ms >= ? ORDER BY id", since);
  p(`events: ${Object.entries(count(cev, (e) => `${e.t} ${e.w} ${e.v}`)).map(([k, v]) => `${k} ×${v}`).join("; ") || "-"}`);
  for (const e of cev.filter((e) => e.t === "quota_refreshed_observed")) {
    let ex = {};
    try {
      ex = JSON.parse(e.e);
    } catch {}
    p(`  ${ts(e.at)} ${e.w} refreshed: ${JSON.stringify(ex).slice(0, 160)}`);
  }
  // resetsAt stability per source (§28): /usage has minute precision; 60 s steps are rounding.
  p("resetsAt changes between consecutive observations of the same instance:");
  const wins = q("SELECT s.source src, w.instance_key ik, s.captured_at_ms c, w.resets_at_ms r FROM provider_windows w JOIN provider_snapshots s ON s.id = w.snapshot_id WHERE s.provider='claude' AND s.captured_at_ms >= ? AND w.resets_at_ms IS NOT NULL ORDER BY w.instance_key, s.captured_at_ms", since);
  const per = {};
  for (let i = 1; i < wins.length; i++) {
    const [x, y] = [wins[i - 1], wins[i]];
    if (x.ik !== y.ik || x.src !== y.src) continue;
    const d = Math.abs(y.r - x.r) / 1000;
    const b = (per[y.src] ??= { "0 s": 0, "≤1 s": 0, "60 s (minute rounding)": 0, "other": 0 });
    b[d === 0 ? "0 s" : d <= 1 ? "≤1 s" : d === 60 ? "60 s (minute rounding)" : "other"]++;
  }
  for (const [src, b] of Object.entries(per)) p(`  ${src.padEnd(10)} ${Object.entries(b).map(([k, v]) => `${k}: ${v}`).join("  ")}`);
}

// ------------------------------------------------------------------------------- notifications
h1("Notification audit");
if (has("notification_log")) {
  const notes = q("SELECT id, created_at_ms t, provider pr, rule_ids_json r, title, delivered d FROM notification_log WHERE created_at_ms >= ? ORDER BY id", since);
  const keys = [];
  for (const n of notes) {
    let ids = [];
    try {
      ids = JSON.parse(n.r);
    } catch {}
    for (const k of ids) keys.push({ k, n });
  }
  const rule = (k) => k.split(":")[0] + (k.includes("claude") ? " (claude)" : "");
  p(`notifications: ${notes.length} (delivered ${notes.filter((n) => n.d === 1).length}); cross-provider combined: ${notes.filter((n) => n.pr === "multi").length}`);
  p(`by provider: ${Object.entries(count(notes, (n) => n.pr)).map(([k, v]) => `${k} ${v}`).join(", ") || "-"}`);
  p(`by rule: ${Object.entries(count(keys, (x) => rule(x.k))).map(([k, v]) => `${k} ${v}`).join(", ") || "-"}`);
  const dup = Object.entries(count(keys, (x) => x.k)).filter(([, v]) => v > 1);
  p(`duplicate rule+scope notifications: ${dup.length}${dup.map(([k, v]) => `\n  DUPLICATE ×${v}: ${k}`).join("")}`);
  notes.slice(-10).forEach((n) => p(`  ${ts(n.t)}  ${n.pr}  "${n.title}"`));
}

// ------------------------------------------------------------------------------ shadow estimator
h1("Codex 5h shadow estimator (diagnostics only)");
p(`idle→active re-anchors: ${reanchors.length}`);
if (has("analysis_diagnostics")) {
  const shadows = q("SELECT created_at_ms t, data_json d FROM analysis_diagnostics WHERE kind='shadow_estimate' AND created_at_ms >= ? ORDER BY id", since).map((r) => ({ t: r.t, ...JSON.parse(r.d) }));
  for (const r of reanchors) {
    const d = JSON.parse(r.d);
    const seg = shadows.filter((s) => s.originMs === d.originMs);
    const last = seg[seg.length - 1];
    const f = (v) => (v == null ? "-" : v.toFixed(1));
    p(`  origin ${ts(d.originMs)} (derived), first active snapshot #${d.segmentStartSnapshotId} at ${d.latestUsedPercent}%`);
    if (last) {
      const div = seg.filter((s) => s.rates.production != null && s.rates.averageSinceOrigin != null).map((s) => s.rates.production / Math.max(s.rates.averageSinceOrigin, 0.01));
      p(`    ${seg.length} polls; last: production ${f(last.rates.production)} pp/h, average ${f(last.rates.averageSinceOrigin)} pp/h, recent 30 min ${f(last.rates.recent30m)} pp/h (${last.latestUsedPercent}%)`);
      if (div.length) p(`    production / average: min ${Math.min(...div).toFixed(2)}, max ${Math.max(...div).toFixed(2)}`);
    }
  }
}

// ---------------------------------------------------------------------------------- watchdog
h1("Watchdog (MONITORING_GAP, database lock)");
const mg = hk("monitoring_gap");
p(`MONITORING_GAP opened: ${mg.filter((g) => g.change === "opened").length} (codex ${mg.filter((g) => g.change === "opened" && g.provider === "codex").length}, claude ${mg.filter((g) => g.change === "opened" && g.provider === "claude").length}); closed: ${mg.filter((g) => g.change === "closed").length}`);
for (const g of mg.filter((g) => g.change === "closed")) p(`  ${g.provider} ${ts(g.sinceMs)} +${mins(g.durationMs)} ${classify(g.sinceMs, g.sinceMs + g.durationMs, 0, g.consecutiveFailures > 0)}${g.readInFlight ? " (read in flight)" : ""}`);
const openGaps = mg.filter((g) => g.change === "opened" && !mg.some((c) => c.change === "closed" && c.provider === g.provider && c.sinceMs === g.sinceMs));
for (const g of openGaps) p(`  STILL OPEN: ${g.provider} since ${ts(g.sinceMs)}`);
p(`database lock held for ≥ 3 watchdog ticks: ${hk("db_busy").length}`);

// ---------------------------------------------------------------------- desktop widget (3.7)
const ws = summarizeWidget(health, since, now);
if (ws) {
  h1("Desktop widget");
  const kv = (o) => Object.entries(o).map(([k, v]) => `${k} ${v}`).join(", ") || "-";
  p(`events: ${kv(ws.events)}; destroyed by reason: ${kv(ws.destroyedBy)}`);
  p(`widget windows at once: max ${ws.maxWidgetWindows}; "created" while one was open: ${ws.createdWhileOpen} (single instance: both must stay ≤ 1 / 0)`);
  p(`shown ${(ws.shownMs / 3_600_000).toFixed(1)} h, of which main window hidden ${(ws.shownMainHiddenMs / 3_600_000).toFixed(1)} h`);
  p(`placements: restored ${ws.placements.restored}, recovered ${ws.placements.recovered}, default ${ws.placements.default}; always on top: on ${ws.alwaysOnTop.on}, off ${ws.alwaysOnTop.off}`);
}

// --------------------------------------------------------------------------------- resources
h1("Resources");
const res = hk("resources");
p(`samples: ${res.length}`);
if (res.length) {
  const series = (f) => res.map((r) => ({ t: r.t, v: f(r) })).filter((x) => x.v != null);
  const slopePerDay = (s) => {
    if (s.length < 3) return null;
    const t0 = s[0].t;
    const xs = s.map((x) => (x.t - t0) / 86_400_000);
    const mx = xs.reduce((a, b) => a + b, 0) / xs.length;
    const my = s.reduce((a, x) => a + x.v, 0) / s.length;
    let sxx = 0;
    let sxy = 0;
    s.forEach((x, i) => {
      sxx += (xs[i] - mx) ** 2;
      sxy += (xs[i] - mx) * (x.v - my);
    });
    return sxx ? sxy / sxx : null;
  };
  const row = (label, f, fmt = mb) => {
    const s = series(f);
    if (!s.length) return;
    const vals = s.map((x) => x.v);
    const sl = slopePerDay(s);
    p(`  ${label.padEnd(26)} first ${fmt(vals[0])}  last ${fmt(vals[vals.length - 1])}  min ${fmt(Math.min(...vals))}  max ${fmt(Math.max(...vals))}  trend ${sl == null ? "-" : fmt(sl) + "/day"}`);
  };
  row("quotadock-app working set", (r) => r.app?.ws);
  row("quotadock-app private", (r) => r.app?.private);
  row("quotadock-app handles", (r) => r.app?.handles, (v) => `${Math.round(v)}`);
  row("WebView2 total working set", (r) => r.webview?.ws);
  row("WebView2 processes", (r) => r.webview?.count, (v) => `${Math.round(v)}`);
  row("codex app-server working set", (r) => r.codex?.ws);
  for (const fg of [false, true]) {
    const s = res.filter((r) => r.foreground === fg);
    const w = res.filter((r) => r.foreground === fg).map((r) => r.webview?.ws).filter((v) => v != null);
    if (w.length) p(`  WebView2 while ${fg ? "window shown" : "in tray"}: median ${mb(w.sort((a, b) => a - b)[Math.floor(w.length / 2)])} (${s.length} samples)`);
  }
  const cpu = [];
  for (let i = 1; i < res.length; i++) {
    const dt = res[i].t - res[i - 1].t;
    const da = (res[i].app?.cpuMs ?? 0) - (res[i - 1].app?.cpuMs ?? 0);
    if (dt > 0 && da >= 0 && dt < 2 * 3_600_000) cpu.push({ fg: res[i].foreground, v: (100 * da) / dt });
  }
  for (const fg of [false, true]) {
    const c = cpu.filter((x) => x.fg === fg).map((x) => x.v);
    if (c.length) p(`  quotadock-app CPU while ${fg ? "window shown" : "in tray"}: average ${(c.reduce((a, b) => a + b, 0) / c.length).toFixed(3)}% of one core`);
  }
  p(`  orphan candidates (codex.exe / claude.exe whose parent is gone, any origin): max ${Math.max(...res.map((r) => r.orphanCandidates?.codex ?? 0))} / ${Math.max(...res.map((r) => r.orphanCandidates?.claude ?? 0))}`);
  p(`  claude.exe children seen during samples: max ${Math.max(...res.map((r) => r.claude?.count ?? 0))}`);
}

console.log(out.join("\n"));
