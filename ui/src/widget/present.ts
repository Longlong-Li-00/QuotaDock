// Widget presentation: wording and formatting only. State, severity, tone and availability arrive
// decided by the backend (WidgetView); nothing here looks at a percentage to judge it.

import type { Dict } from "../i18n/en.ts";
import type { DisplayTone, Lang, Severity, WidgetProvider, WidgetReset, WidgetStaleKind, WidgetView } from "../api/types.ts";
import { EXACT_PRECISION_MS, formatDay, formatTime, resetPhrase, sameDay, weekdayIsClear } from "../model/present.ts";

export type WTone = "normal" | "warning" | "critical" | "muted";

const MIN = 60_000;

function severityName(sev: Severity, t: Dict): string {
  switch (sev) {
    case "warning":
      return t.sevWarning;
    case "critical":
      return t.sevCritical;
    case "error":
      return t.sevError;
    default:
      return "";
  }
}

/** Remaining percent (the widget never shows used percent); no value -> "—". */
export function remainingText(v: number | null): string {
  return v === null ? "—" : `${Math.round(v)}%`;
}

/** Bar width for a remaining percent, kept inside 0-100. */
export function barWidth(v: number | null): number {
  return v === null ? 0 : Math.max(0, Math.min(100, v));
}

export function ageText(atMs: number | null, now: number, t: Dict): string | null {
  if (atMs === null) return null;
  const m = Math.max(0, Math.floor((now - atMs) / MIN));
  return m < 120 ? t.wAgeMinutes(m) : t.wAgeHours(Math.floor(m / 60));
}

/**
 * The reset inside a compact note. Next to two value columns the note has about 76 px after the name
 * (measured), too little for the widget phrase: the clock while the reset is today, the bare day or date
 * once it is not, where an approximate time needs no mark because the minutes are gone. A clock alone
 * would read as already gone once the reset is tomorrow. A hidden 5-hour column leaves room for the full
 * phrase. Only a confirmed reset reaches a clock here too.
 */
function noteWhen(r: WidgetReset, roomy: boolean, now: number, t: Dict, lang: Lang): string {
  if (r.schedule === "sliding") return t.wNoteIdle;
  if (r.schedule === "unconfirmed") return t.wNoteUnconfirmed;
  if (roomy || r.schedule !== "confirmed" || r.atMs === null || r.atMs <= now) return resetPhrase(r, "widget", now, t, lang);
  if (!sameDay(r.atMs, now)) return weekdayIsClear(r.atMs, now) ? t.weekdays[new Date(r.atMs).getDay()]! : formatDay(r.atMs, lang);
  const clock = formatTime(r.atMs, lang);
  return r.precisionMs !== null && r.precisionMs > EXACT_PRECISION_MS ? t.wResetNear(clock) : clock;
}

const isLow = (tone: DisplayTone) => tone === "low" || tone === "critical";

/** The compact note for a low window: the shown low window with the least remaining, ties to 5-hour. */
function lowNote(p: WidgetProvider, now: number, t: Dict, lang: Lang): { text: string; tone: WTone } | null {
  const five = p.showFiveHour && isLow(p.fiveHourTone) ? p.fiveHourRemaining : null;
  const weekly = isLow(p.weeklyTone) ? p.weeklyRemaining : null;
  if (five === null && weekly === null) return null;
  const useFive = five !== null && (weekly === null || five <= weekly);
  const reset = useFive ? p.fiveHourReset : p.weeklyReset;
  const when = noteWhen(reset, !p.showFiveHour, now, t, lang);
  const tone = useFive ? p.fiveHourTone : p.weeklyTone;
  return { text: `${useFive ? t.wFiveHour : t.wWeekly} ${when}`, tone: p.state === "paused" ? "muted" : tone === "critical" ? "critical" : "warning" };
}

/** How each kind of old data is said: amber may be stale, red stale, and quiet for a passive source's normal state. */
const STALE_TONE: Record<WidgetStaleKind, WTone> = { maybe: "warning", stale: "critical", not_live: "muted" };

/** The time of the last data today, its date before that; the short forms for the compact slot have no room for a date. */
function staleWords(kind: WidgetStaleKind, at: number, now: number, t: Dict, lang: Lang): { text: string; short: string } {
  const today = sameDay(at, now);
  const when = today ? formatTime(at, lang) : formatDay(at, lang);
  switch (kind) {
    case "maybe":
      return { text: t.wMaybeStale(when), short: t.wMaybeStaleShort };
    case "not_live":
      return { text: t.wNotLive(when), short: today ? t.wNotLiveShort(when) : t.wNotLiveOld };
    case "stale":
      return { text: t.wStale(when), short: today ? t.wStaleShort(when) : t.wStaleOld };
  }
}

export interface RowView {
  id: string;
  name: string;
  five: string;
  weekly: string;
  fiveValue: number | null;
  weeklyValue: number | null;
  /** That window's tone from the backend; paused mutes every value. */
  fiveTone: DisplayTone;
  weeklyTone: DisplayTone;
  /** The widget's reset phrase for each window (detailed layout). */
  fiveReset: string;
  weeklyReset: string;
  /** False: no 5-hour cell or bar at all ("—" means shown, but no value). */
  showFive: boolean;
  /** Short text next to the name in the detailed layout (waiting, stale, limit reached, error). */
  status: string | null;
  statusTone: WTone;
  /** The one note in the compact name slot: a status first, else the reset of a low window. */
  note: { text: string; tone: WTone } | null;
  /** An error with no values at all: the message may use the whole row (compact layout). */
  wide: boolean;
  /** Accessible name / tooltip with everything the row shows, and more: severity and every reset. */
  label: string;
}

export function rowView(p: WidgetProvider, now: number, t: Dict, lang: Lang): RowView {
  let status: string | null = null;
  let statusShort: string | null = null;
  let statusTone: WTone = "muted";
  const staleKind = p.staleKind ?? "stale"; // only read where the state says old data
  const aged = p.updatedAtMs === null ? null : staleWords(staleKind, p.updatedAtMs, now, t, lang);
  switch (p.state) {
    case "live":
    case "paused": // the header says "Paused"; the last values stay visible, muted
      break;
    case "stale":
      // A provider that names why it is not live (Claude: not set up, not signed in) says that, as the
      // tray does; plain old data says how old.
      if (p.note !== null && p.note !== t.wNotLiveOld) {
        status = p.note;
        statusShort = p.noteShort;
      } else {
        status = aged?.text ?? p.note;
        statusShort = aged?.short ?? null;
      }
      statusTone = STALE_TONE[staleKind];
      break;
    case "waiting":
      // A cause the backend names, else the true wait. No data is quiet either way.
      status = p.waitingNote ?? t.wWaiting;
      break;
    case "error":
      // With no reason from the backend the last data's time says more than "Error".
      status = p.note ?? aged?.text ?? t.wError;
      statusShort = p.note === null ? (aged?.short ?? null) : null;
      statusTone = "critical";
      break;
  }
  // Only the backend's availability (Codex: ordinaryUsageAllowed) may say "limit reached". Its note is
  // those words, or a provider error's, which takes the row from them.
  const blocked = p.availability === "blocked";
  const limit = blocked ? (p.note ?? t.limitReached) : null;
  if (limit !== null) {
    // A limit from old data says so where there is room, as the tray and Overview do. Paused has its own header.
    const old = limit === t.limitReached && p.state !== "paused" && p.staleKind !== null ? aged?.short : null;
    status = old ? `${limit} · ${old}` : limit;
    statusShort = null;
    statusTone = "critical";
  }
  const five = remainingText(p.fiveHourRemaining);
  const weekly = remainingText(p.weeklyRemaining);
  const fiveKnown = p.showFiveHour && p.fiveHourRemaining !== null;
  const muted = p.state === "paused";
  const comma = lang === "zh-CN" ? "，" : ", ";
  const spoken = (v: number | null, r: WidgetReset) => `${v === null ? t.wNoValue : t.wRemaining(Math.round(v))}${comma}${resetPhrase(r, "overview", now, t, lang)}`;
  // The row has one note, so the tooltip always names a limit and how old the data behind it is.
  const blockedAge = blocked && p.staleKind !== null ? aged?.text : null;
  const extra = [...new Set([severityName(p.severity, t), limit ?? status, blocked ? t.limitReached : null, blockedAge])].filter((x): x is string => !!x).join(comma);
  // Old data with no values keeps its dashes; only an error or a limit takes over the row.
  const wide = !fiveKnown && p.weeklyRemaining === null && statusTone === "critical" && p.state !== "stale";
  // The compact slot beside values: a backend note's short form on any such row, the age's only beside two columns.
  const noteShort = status === p.note || status === p.waitingNote ? p.noteShort : null;
  const slot = wide ? null : (noteShort ?? (p.showFiveHour ? statusShort : null));
  return {
    id: p.id,
    name: p.displayName,
    five,
    weekly,
    fiveValue: p.fiveHourRemaining,
    weeklyValue: p.weeklyRemaining,
    fiveTone: muted ? "muted" : p.fiveHourTone,
    weeklyTone: muted ? "muted" : p.weeklyTone,
    fiveReset: resetPhrase(p.fiveHourReset, "widget", now, t, lang),
    weeklyReset: resetPhrase(p.weeklyReset, "widget", now, t, lang),
    showFive: p.showFiveHour,
    status,
    statusTone,
    note: status === null ? lowNote(p, now, t, lang) : { text: slot ?? limit ?? status, tone: statusTone },
    wide,
    label: t.wProviderLabel(p.displayName, p.showFiveHour ? spoken(p.fiveHourRemaining, p.fiveHourReset) : null, spoken(p.weeklyRemaining, p.weeklyReset), extra),
  };
}

/** The compact header's status: "Paused" or nothing. The ring layout has no header; each row says it. */
export function headerStatus(v: WidgetView, _now: number, t: Dict): { text: string; tone: WTone } | null {
  if (v.providers.length === 0) return null; // no header at all: only the hint
  return v.paused ? { text: t.wPaused, tone: "warning" } : null;
}

/** One provider in the detailed (ring) layout: weekly on the outer ring and in the big number, 5-hour on the inner ring and its own line. */
export interface RingView {
  id: string;
  name: string;
  weekly: string;
  weeklyValue: number | null;
  weeklyTone: DisplayTone;
  /** Null: the 5-hour window is hidden, so there is no inner ring and no 5-hour line. */
  five: { text: string; value: number | null; tone: DisplayTone; reset: string | null } | null;
  /** The line under the number: the row's status when it has one, else the weekly reset. */
  line: { text: string; tone: WTone };
  label: string;
}

/** The 5-hour line's reset: a countdown as the widget says it; an expected time as a bare clock after a "~" (the reset is within 5 hours, and "预计 周四 00:35" does not fit beside the value). */
function fiveResetWords(r: WidgetReset, phrase: string, now: number, lang: Lang): string {
  const coarse = r.precisionMs !== null && r.precisionMs > EXACT_PRECISION_MS;
  return coarse && r.atMs !== null ? `~${formatTime(r.atMs, lang)}` : phrase;
}

export function ringView(p: WidgetProvider, now: number, t: Dict, lang: Lang): RingView {
  const r = rowView(p, now, t, lang);
  // The line has the width of the text beside the ring, so a backend note takes its short form where
  // it has one (the provider's name is right above it) and a limit the short words.
  const shortNote = (s: string) => ((s === p.note || s === p.waitingNote) && p.noteShort !== null ? p.noteShort : s);
  let line: { text: string; tone: WTone };
  if (p.availability === "blocked") {
    // As in rowView: only the plain limit says how old its data is; a provider error takes the line.
    const plain = p.note === null || p.note === t.limitReached;
    const aged = plain && p.state !== "paused" && p.staleKind !== null && p.updatedAtMs !== null ? staleWords(p.staleKind, p.updatedAtMs, now, t, lang).short : null;
    const words = plain ? t.wAtLimit : shortNote(p.note!);
    line = { text: aged ? `${words} · ${aged}` : words, tone: "critical" };
  } else if (r.status !== null) {
    line = { text: shortNote(r.status), tone: r.statusTone };
  } else if (p.state === "paused") {
    line = { text: t.wPaused, tone: "muted" }; // no header in this layout
  } else {
    line = { text: resetPhrase(p.weeklyReset, "ring", now, t, lang), tone: "muted" };
  }
  // Only a time left fits after the 5-hour value; the other reset words are in the tooltip.
  const fiveAt = p.fiveHourReset.schedule === "confirmed" && p.fiveHourReset.atMs !== null && p.fiveHourReset.atMs > now;
  return {
    id: r.id,
    name: r.name,
    weekly: r.weekly,
    weeklyValue: r.weeklyValue,
    weeklyTone: r.weeklyTone,
    five: r.showFive ? { text: r.five, value: r.fiveValue, tone: r.fiveTone, reset: r.fiveValue !== null && fiveAt ? fiveResetWords(p.fiveHourReset, r.fiveReset, now, lang) : null } : null,
    line,
    label: r.label,
  };
}
