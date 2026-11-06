// Browser-only mock for widget design previews (never used inside Tauri). Fixed scenarios:
// widget.html?w=normal|low|critical|blocked|sliding|unconfirmed|passed|passedread|missing|approx|stale|staleold|maybestale|notlive|staleerror|waiting|paused|error|signedout|unavailable|notfound|limitunavailable|limitsignedout|limitmaybe|limitstale|limitold|limitpaused|failing|claudenotsetup|claudeapibilling|claudenotfound|claudenotauth|claudeverify|claudeerror|codexonly|claudeonly|none
//   &mode=compact|detailed &theme=system|light|dark &lang=en|zh-CN &opacity=50..100 &locked=1
//   &five=codex|claude|both (whose 5-hour limit is hidden)
//   &icons=1 (stand-in custom ring icons, as files in the data folder's icons folder would give)

import { mockTone } from "../api/mockTone.ts";
import type { Lang, Severity, ThemePref, WidgetProvider, WidgetReset, WidgetView } from "../api/types.ts";
import type { WidgetApi } from "./api.ts";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** Backend notes arrive translated; the mock carries both languages for the few it uses. */
const NOTES: Record<string, Record<Lang, string>> = {
  limit: { en: "Limit reached", "zh-CN": "已达到限制" },
  signedOut: { en: "Sign in to Codex first", "zh-CN": "请先登录 Codex" },
  signedOutShort: { en: "Sign in first", "zh-CN": "请先登录" },
  unavailable: { en: "Cannot refresh Codex usage", "zh-CN": "无法刷新 Codex 用量" },
  unavailableShort: { en: "Can't refresh", "zh-CN": "无法刷新" },
  codexNotFound: { en: "Codex installation not found", "zh-CN": "未找到 Codex" },
  notInstalled: { en: "Not installed", "zh-CN": "未安装" },
  notSetUp: { en: "Not set up", "zh-CN": "未设置" },
  apiBilling: { en: "API billing", "zh-CN": "API 计费" },
  notSignedIn: { en: "Not signed in", "zh-CN": "未登录" },
  needsVerification: { en: "Needs verification", "zh-CN": "需要验证" },
  cannotRead: { en: "Cannot read data", "zh-CN": "无法读取数据" },
  notLive: { en: "Not live", "zh-CN": "非实时" },
};

/** Any reset but a confirmed one: no time. */
function noTime(schedule: Exclude<WidgetReset["schedule"], "confirmed">): WidgetReset {
  return { schedule, atMs: null, precisionMs: null };
}

function provider(now: number, id: "codex" | "claude", five: number | null, weekly: number | null, over: Partial<WidgetProvider>): WidgetProvider {
  // Claude's status line gives seconds; Codex has no precision. Weekly: one weekday form, one date form.
  const confirmed = (inMs: number): WidgetReset => ({ schedule: "confirmed", atMs: now + inMs, precisionMs: id === "claude" ? 1000 : null });
  const fiveHourReset = over.fiveHourReset ?? (five === null ? noTime("missing") : confirmed(id === "codex" ? 2 * HOUR + 13 * MIN : 3 * HOUR + 20 * MIN));
  const weeklyReset = over.weeklyReset ?? (weekly === null ? noTime("missing") : confirmed(id === "codex" ? 3 * DAY + 2 * HOUR : 7 * DAY));
  // Codex's "stale" state is only ever "may be stale", which still counts as current; a passed reset has no tone.
  const state = over.state ?? "live";
  const current = state === "live" || (id === "codex" && state === "stale");
  return {
    id,
    displayName: id === "codex" ? "Codex" : "Claude",
    shortLabel: id === "codex" ? "C" : "A",
    fiveHourRemaining: five,
    weeklyRemaining: weekly,
    fiveHourTone: mockTone(fiveHourReset.schedule === "passed" ? null : five, current),
    weeklyTone: mockTone(weeklyReset.schedule === "passed" ? null : weekly, current),
    fiveHourReset,
    weeklyReset,
    showFiveHour: true,
    state,
    staleKind: null,
    availability: id === "codex" ? "allowed" : "unknown",
    severity: "normal",
    updatedAtMs: null,
    note: null,
    noteShort: null,
    waitingNote: null,
    ...over,
  };
}

export function mockView(params: URLSearchParams, now: number): WidgetView {
  const scenario = params.get("w") ?? "normal";
  const lang: Lang = params.get("lang") === "zh-CN" || (params.get("lang") === null && navigator.language.startsWith("zh")) ? "zh-CN" : "en";
  const note = (k: string) => NOTES[k]?.[lang] ?? k;
  // Claude with no data for a reason its state names; the short form only where the full words do not fit.
  const claudeNoData = (k: string, short: string | null = null) => provider(now, "claude", null, null, { state: "waiting", note: note(k), waitingNote: note(k), noteShort: short });
  // Codex reads fail but the last one is recent, so its values are still current and keep their tones.
  const failingWithData = (five: number, weekly: number, over: Partial<WidgetProvider>) =>
    provider(now, "codex", five, weekly, { state: "error", severity: "error", updatedAtMs: now - 2 * MIN, fiveHourTone: mockTone(five, true), weeklyTone: mockTone(weekly, true), ...over });
  const fresh = now - 40_000;
  const claudeAt = now - 2 * MIN;
  let codex = provider(now, "codex", 88, 82, { updatedAtMs: fresh });
  let claude: WidgetProvider | null = provider(now, "claude", 61, 72, { updatedAtMs: claudeAt });
  let paused = false;
  switch (scenario) {
    case "low":
      // Codex's lowest is weekly; Claude's two windows tie, so a note would name 5-hour.
      codex = provider(now, "codex", 28, 22, { updatedAtMs: fresh, severity: "warning" });
      claude = provider(now, "claude", 24, 24, { updatedAtMs: claudeAt, severity: "warning" });
      break;
    case "critical":
      // Codex's 4 % has its reminder off: the tone is critical, only the weekly window raises severity.
      codex = provider(now, "codex", 4, 62, { updatedAtMs: fresh });
      claude = provider(now, "claude", 36, 8, { updatedAtMs: claudeAt, severity: "critical" });
      break;
    case "blocked":
      codex = provider(now, "codex", 0, 40, { updatedAtMs: fresh, availability: "blocked", severity: "critical", note: note("limit"), fiveHourReset: { schedule: "confirmed", atMs: now + 47 * MIN, precisionMs: null } });
      break;
    case "sliding":
      codex = provider(now, "codex", 100, 98, { updatedAtMs: fresh, fiveHourReset: noTime("sliding") });
      break;
    case "unconfirmed":
      codex = provider(now, "codex", 88, 22, { updatedAtMs: fresh, severity: "warning", fiveHourReset: noTime("unconfirmed"), weeklyReset: noTime("unconfirmed") });
      claude = provider(now, "claude", 24, 72, { updatedAtMs: claudeAt, fiveHourReset: noTime("unconfirmed") });
      break;
    case "passed":
      // The value from before a passed reset belongs to the last period: neither provider shows one.
      codex = provider(now, "codex", null, 82, { updatedAtMs: fresh, fiveHourReset: noTime("passed") });
      claude = provider(now, "claude", null, 72, { updatedAtMs: claudeAt, fiveHourReset: { schedule: "passed", atMs: null, precisionMs: 1000 } });
      break;
    case "passedread":
      // Codex read after that time: the service's current answer keeps its number, with no tone.
      codex = provider(now, "codex", 13, 82, { updatedAtMs: fresh, fiveHourReset: noTime("passed") });
      break;
    case "missing":
      codex = provider(now, "codex", null, 82, { updatedAtMs: fresh });
      claude = provider(now, "claude", 61, null, { updatedAtMs: claudeAt });
      break;
    case "approx": {
      // `claude -p /usage`: to the minute. One today, one a few days out.
      const minute = (inMs: number): WidgetReset => ({ schedule: "confirmed", atMs: Math.floor((now + inMs) / MIN) * MIN, precisionMs: MIN });
      claude = provider(now, "claude", 24, 72, { updatedAtMs: now - 6 * MIN, fiveHourReset: minute(3 * HOUR + 20 * MIN), weeklyReset: minute(3 * DAY + 2 * HOUR) });
      break;
    }
    case "stale":
      // A high value goes muted for Claude, a low one stays low; Codex is only "may be stale".
      codex = provider(now, "codex", 82, 22, { state: "stale", staleKind: "maybe", severity: "warning", updatedAtMs: now - 14 * MIN });
      claude = provider(now, "claude", 61, 24, { state: "stale", staleKind: "not_live", severity: "warning", updatedAtMs: now - 47 * MIN, note: note("notLive") });
      break;
    case "maybestale":
      codex = provider(now, "codex", 82, 64, { state: "stale", staleKind: "maybe", updatedAtMs: now - 14 * MIN });
      break;
    case "notlive":
      // The resting state of passive Claude data: quiet values, quiet words, no alarm.
      claude = provider(now, "claude", 61, 72, { state: "stale", staleKind: "not_live", updatedAtMs: now - 47 * MIN, note: note("notLive") });
      break;
    case "staleerror":
      // Codex with no valid read for 30 min: an error by the rules, its last values kept and muted or low.
      codex = provider(now, "codex", 82, 22, { state: "error", staleKind: "stale", availability: "allowed", severity: "error", updatedAtMs: now - 47 * MIN });
      break;
    case "waiting":
      claude = provider(now, "claude", null, null, { state: "waiting" });
      break;
    case "paused":
      paused = true;
      codex = provider(now, "codex", 88, 22, { state: "paused", severity: "warning", updatedAtMs: now - 12 * MIN });
      claude = provider(now, "claude", 61, 72, { state: "paused", updatedAtMs: now - 14 * MIN });
      break;
    case "error":
      codex = provider(now, "codex", null, null, { state: "error", availability: "unknown", severity: "error", note: note("signedOut"), noteShort: note("signedOutShort") });
      break;
    case "signedout":
      // Values beside the note: the short form is for the compact slot, the whole sentence for the rest.
      codex = failingWithData(88, 82, { note: note("signedOut"), noteShort: note("signedOutShort") });
      break;
    case "unavailable":
      codex = failingWithData(88, 82, { note: note("unavailable"), noteShort: note("unavailableShort") });
      break;
    case "notfound":
      codex = provider(now, "codex", null, null, { state: "error", availability: "unknown", severity: "error", note: note("codexNotFound"), noteShort: note("notInstalled") });
      break;
    case "limitunavailable":
      codex = failingWithData(0, 40, { availability: "blocked", note: note("unavailable"), noteShort: note("unavailableShort") });
      break;
    case "limitsignedout":
      codex = failingWithData(0, 40, { availability: "blocked", note: note("signedOut"), noteShort: note("signedOutShort") });
      break;
    case "limitmaybe":
      // A limit from old data: detailed says how old, compact keeps the limit's words alone.
      codex = provider(now, "codex", 0, 40, { state: "stale", staleKind: "maybe", availability: "blocked", severity: "critical", note: note("limit"), updatedAtMs: now - 14 * MIN });
      break;
    case "limitstale":
      codex = provider(now, "codex", 0, 40, { state: "error", staleKind: "stale", availability: "blocked", severity: "error", note: note("limit"), updatedAtMs: now - 47 * MIN });
      break;
    case "limitold":
      // Older than today: the row gives no time. The 5-hour reset has gone by, so that value is from the last period.
      codex = provider(now, "codex", null, 40, { state: "error", staleKind: "stale", availability: "blocked", severity: "error", note: note("limit"), updatedAtMs: now - 3 * DAY, fiveHourReset: noTime("passed") });
      break;
    case "limitpaused":
      // The header says paused, so the row does not say how old as well.
      paused = true;
      codex = provider(now, "codex", 0, 40, { state: "paused", staleKind: "stale", availability: "blocked", severity: "critical", note: note("limit"), updatedAtMs: now - 3 * HOUR });
      claude = provider(now, "claude", 61, 72, { state: "paused", updatedAtMs: now - 14 * MIN });
      break;
    case "staleold":
      // No valid read for days: the date in the whole words, none in the short form.
      codex = provider(now, "codex", 82, 22, { state: "error", staleKind: "stale", severity: "error", updatedAtMs: now - 3 * DAY });
      break;
    case "failing":
      // Every poll fails and nothing was ever stored: still a waiting row, with the cannot-refresh word.
      codex = provider(now, "codex", null, null, { state: "waiting", availability: "unknown", waitingNote: note("unavailableShort") });
      break;
    case "claudenotsetup":
      claude = claudeNoData("notSetUp");
      break;
    case "claudeapibilling":
      claude = claudeNoData("apiBilling");
      break;
    case "claudenotfound":
      claude = claudeNoData("notInstalled");
      break;
    case "claudenotauth":
      claude = claudeNoData("notSignedIn");
      break;
    case "claudeverify":
      claude = claudeNoData("needsVerification", lang === "en" ? "Needs check" : null);
      break;
    case "claudeerror":
      claude = claudeNoData("cannotRead", lang === "en" ? "Can't read" : null);
      break;
    case "codexonly":
      claude = null;
      break;
  }
  const hidden = params.get("five");
  if (hidden === "codex" || hidden === "both") codex = { ...codex, showFiveHour: false };
  if (claude && (hidden === "claude" || hidden === "both")) claude = { ...claude, showFiveHour: false };
  const providers = scenario === "none" ? [] : scenario === "claudeonly" && claude ? [claude] : claude ? [codex, claude] : [codex];
  const order: Severity[] = ["normal", "warning", "critical", "error"];
  const globalSeverity = providers.reduce<Severity>((acc, p) => (order.indexOf(p.severity) > order.indexOf(acc) ? p.severity : acc), "normal");
  const updated = providers.map((p) => p.updatedAtMs).filter((x): x is number => x !== null);
  const opacity = Math.max(50, Math.min(100, Number(params.get("opacity") ?? 85) || 85));
  return {
    generatedAtMs: now,
    paused,
    globalSeverity,
    language: lang,
    theme: (params.get("theme") as ThemePref | null) ?? "system",
    mode: params.get("mode") === "detailed" ? "detailed" : "compact",
    opacity,
    locked: params.get("locked") === "1",
    providers,
    updatedAtMs: updated.length ? Math.max(...updated) : null,
    customIcons: params.get("icons") === "1" ? STAND_IN_ICONS : {},
  };
}

/** Plain shapes, not any company's mark: they only show where a user's own icon would sit. */
const standIn = (body: string) => `data:image/svg+xml;base64,${btoa(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">${body}</svg>`)}`;
const STAND_IN_ICONS: Record<string, string> = {
  codex: standIn(`<rect x="3" y="3" width="18" height="18" rx="5" fill="#2f6fd6"/>`),
  claude: standIn(`<circle cx="12" cy="12" r="9" fill="#c2410c"/>`),
};

export const mockWidgetApi: WidgetApi = {
  getView: async () => mockView(new URLSearchParams(location.search), Date.now()),
  openMain: async () => console.info("[widget preview] would open the main window"),
  startDrag: async () => false,
  contextMenu: async () => console.info("[widget preview] would show the native menu"),
};
