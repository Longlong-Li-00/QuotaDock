// Pure presentation mapping. Every decision here is about wording/formatting of values the Rust
// core already decided (schedule state, availability, risk, freshness). No quota logic lives here.

import type { Dict } from "../i18n/en.ts";
import { lookup } from "../i18n/index.ts";
import type { DisplayTone, EventView, Lang, Risk, Severity, TrayStatusModel, UsageForecast, WidgetReset, WindowStatus, WindowType } from "../api/types.ts";

export type Tone = "ok" | "warning" | "critical" | "unknown" | "neutral";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
/** Claude status line: to the second. Anything coarser is an approximate time. */
export const EXACT_PRECISION_MS = 1000;

export function availabilityView(allowed: boolean | null, persistentUnknown: boolean, t: Dict): { label: string; tone: Tone } {
  if (allowed === true) return { label: t.available, tone: "ok" };
  if (allowed === false) return { label: t.limitReached, tone: "critical" };
  return { label: persistentUnknown ? t.availabilityUnknownPersistent : t.availabilityUnknown, tone: persistentUnknown ? "warning" : "unknown" };
}

/** The card's pill exists only for a Codex limit or availability that stays unknown; never for Claude. */
export function availabilityPill(provider: string, allowed: boolean | null, persistentUnknown: boolean, t: Dict): { label: string; tone: Tone } | null {
  if (provider !== "codex" || allowed === true) return null;
  if (allowed === null && !persistentUnknown) return null;
  return availabilityView(allowed, persistentUnknown, t);
}

export function formatDateTime(ms: number, lang: Lang): string {
  const d = new Date(ms);
  const time = d.toLocaleTimeString(lang === "zh-CN" ? "zh-CN" : "en-GB", { hour: "2-digit", minute: "2-digit", hour12: false });
  if (lang === "zh-CN") return `${d.getMonth() + 1}月${d.getDate()}日 ${time}`;
  const month = d.toLocaleString("en-US", { month: "short" });
  return `${month} ${d.getDate()} · ${time}`;
}

export function formatDay(ms: number, lang: Lang): string {
  const d = new Date(ms);
  if (lang === "zh-CN") return `${d.getMonth() + 1}月${d.getDate()}日`;
  return `${d.toLocaleString("en-US", { month: "short" })} ${d.getDate()}`;
}

export function formatTime(ms: number, lang: Lang): string {
  return new Date(ms).toLocaleTimeString(lang === "zh-CN" ? "zh-CN" : "en-GB", { hour: "2-digit", minute: "2-digit", hour12: false });
}

export function sameDay(a: number, b: number): boolean {
  return new Date(a).toDateString() === new Date(b).toDateString();
}

/** Whether a weekday name can only mean the coming one. Today's own weekday, almost a week out, reads as already gone. */
export function weekdayIsClear(at: number, now: number): boolean {
  return at - now < WEEK && new Date(at).getDay() !== new Date(now).getDay();
}

/** A later moment on a clock: today "14:30", within a week "Thu 14:30", further out a date. */
export function whenText(at: number, now: number, t: Dict, lang: Lang, dateOnly = false): string {
  const time = formatTime(at, lang);
  if (sameDay(at, now)) return time;
  if (weekdayIsClear(at, now)) return `${t.weekdays[new Date(at).getDay()]} ${time}`;
  if (dateOnly) return formatDay(at, lang);
  return `${formatDay(at, lang)}${lang === "zh-CN" ? " " : ", "}${time}`;
}

/** Overview: whole sentences. Widget: the compact and 5-hour-line forms. Ring: the widget's short times with the verb, under a ring's number. */
export type ResetForm = "overview" | "widget" | "ring";

/** A window's reset in the shape the backend sends the widget, so both surfaces share one formatter. */
export function windowReset(w: WindowStatus): WidgetReset {
  if (!w.present) return { schedule: "missing", atMs: null, precisionMs: null };
  return { schedule: w.resetSchedule, atMs: w.confirmedResetAtMs, precisionMs: w.resetPrecisionMs };
}

/** The one reset wording. Only a confirmed time reaches a clock or a countdown; sliding, unconfirmed and passed never do. */
export function resetPhrase(r: WidgetReset, form: ResetForm, now: number, t: Dict, lang: Lang): string {
  const widget = form !== "overview";
  const unconfirmed = form === "widget" ? t.wResetUnconfirmed : t.resetUnconfirmed;
  const passed = widget ? t.wResetPassed : t.resetPassed;
  switch (r.schedule) {
    case "missing":
      return widget ? t.wWindowMissing : t.windowMissing;
    case "sliding":
      return widget ? t.wSliding : t.sliding;
    case "unconfirmed":
      return unconfirmed;
    case "passed":
      return passed;
  }
  if (r.atMs === null) return unconfirmed;
  if (r.atMs <= now) return passed; // passed between polls: never a negative countdown
  if (r.precisionMs !== null && r.precisionMs > EXACT_PRECISION_MS) {
    const when = whenText(r.atMs, now, t, lang, widget);
    return form === "widget" ? t.wResetExpected(when) : t.resetExpected(when);
  }
  if (r.atMs - now < DAY) {
    const minutes = Math.max(1, Math.floor((r.atMs - now) / MIN));
    const [h, m] = [Math.floor(minutes / 60), minutes % 60];
    return form === "widget" ? t.wResetsIn(t.wDurationHM(h, m)) : t.resetsIn(widget ? t.wDurationHM(h, m) : t.durationHM(h, m));
  }
  const when = whenText(r.atMs, now, t, lang, widget);
  return form === "widget" ? when : t.resetsOn(when);
}

/** An extra limit's reset as the source reports it: a clock, never a countdown, and "expected" when coarser than a second. */
export function extraResetPhrase(atMs: number, precisionMs: number | null, now: number, t: Dict, lang: Lang): string {
  const when = whenText(atMs, now, t, lang);
  return precisionMs !== null && precisionMs > EXACT_PRECISION_MS ? t.resetExpected(when) : t.resetsOn(when);
}

/** Colour of a window's value. The tone is the backend's; a passed reset stays neutral whatever it says. */
export function windowTone(w: WindowStatus, tone: DisplayTone | undefined): DisplayTone {
  if (w.resetSchedule === "passed") return "muted";
  return tone ?? "unknown";
}

/** The words that go with a low tone, so colour is never the only signal. */
export function toneLabel(tone: DisplayTone, t: Dict): string | null {
  if (tone === "critical" || tone === "empty") return t.toneCritical;
  if (tone === "low") return t.toneLow;
  return null;
}

/** The one forecast line on Overview. No result never means safe: anything but at-risk shows nothing. */
export function forecastLine(f: UsageForecast, now: number, t: Dict, lang: Lang): string | null {
  if (f.risk !== "at_risk" || f.status === "reset_schedule_unconfirmed") return null;
  if (f.serverResetAtMs !== null && f.serverResetAtMs <= now) return null; // that reset has gone by: "before reset" says nothing now
  if (f.remainingPercent === null || f.remainingPercent <= 0) return null;
  const at = f.estimatedExhaustionAtMs;
  if (at === null || at <= now) return t.forecastBeforeReset;
  if (sameDay(at, now)) return t.forecastRunsOut(formatTime(at, lang));
  return t.forecastRunsOut(weekdayIsClear(at, now) ? t.weekdays[new Date(at).getDay()]! : formatDay(at, lang));
}

/** The forecast line under a window's reset line. A reset line saying missing or passed takes none, whatever the forecast still holds. */
export function windowForecastLine(w: WindowStatus, f: UsageForecast, now: number, t: Dict, lang: Lang): string | null {
  const r = windowReset(w);
  if (r.schedule === "missing" || r.schedule === "passed") return null;
  if (r.schedule === "confirmed" && r.atMs !== null && r.atMs <= now) return null;
  return forecastLine(f, now, t, lang);
}

/** Reasons that get the page's one banner; everything else the cards show themselves. */
const BANNER_REASONS: Record<string, Tone> = {
  not_authenticated: "critical",
  provider_unavailable: "critical",
  no_valid_snapshot_30m: "critical",
  limit_reached: "critical",
  availability_unknown_persistent: "warning",
};

/**
 * One plain sentence for the states a card cannot express. Quota low alone gets none. `covered`:
 * providers whose card already has a banner of its own; the line keeps only their limit.
 */
export function topBanner(tray: TrayStatusModel, t: Dict, covered: ReadonlySet<string> = new Set()): { text: string; tone: Tone } | null {
  const shows = (provider: string, reason: string) => reason in BANNER_REASONS && (reason === "limit_reached" || !covered.has(provider));
  const shown = tray.providers.flatMap((p) => p.reasons.filter((r) => shows(p.provider, r)).map((r) => ({ name: p.displayName, reason: r })));
  if (shown.length === 0) return null;
  const sentence = ({ name, reason }: { name: string; reason: string }) => {
    const text = reasonText(reason, t);
    return text.includes(name) ? text : t.bannerLine(name, text);
  };
  const tone = shown.some((s) => BANNER_REASONS[s.reason] === "critical") ? "critical" : "warning";
  return { text: [...new Set(shown.map(sentence))].join(" · "), tone };
}

export function pct(n: number | null): number | null {
  return n === null ? null : Math.round(n);
}

export function freshnessView(tray: TrayStatusModel, now: number, t: Dict): { text: string; tone: Tone } {
  if (tray.updatedAtMs === null) return { text: t.noDataYet, tone: "unknown" };
  const ago = agoText(tray.updatedAtMs, now, t);
  if (tray.paused) return { text: ago, tone: "neutral" }; // no polls are made, so an old age is expected
  switch (tray.freshness) {
    case "stale":
      return { text: `${t.dataStale} · ${ago}`, tone: "critical" };
    case "maybe_stale":
      return { text: `${ago} · ${t.dataMayBeStale}`, tone: "warning" };
    default:
      return { text: ago, tone: "neutral" };
  }
}

export function severityView(sev: Severity, t: Dict): { label: string; tone: Tone } {
  switch (sev) {
    case "error":
      return { label: t.sevError, tone: "critical" };
    case "critical":
      return { label: t.sevCritical, tone: "critical" };
    case "warning":
      return { label: t.sevWarning, tone: "warning" };
    default:
      return { label: t.sevNormal, tone: "ok" };
  }
}

export function reasonText(code: string, t: Dict): string {
  return lookup(t, `reason_${code}`, code);
}

export function riskView(risk: Risk, t: Dict): { label: string; tone: Tone } {
  switch (risk) {
    case "at_risk":
      return { label: t.riskAtRisk, tone: "critical" };
    case "watch":
      return { label: t.riskWatch, tone: "warning" };
    case "low":
      return { label: t.riskLow, tone: "neutral" }; // No green at rest; a low risk is not a promise.
    default:
      return { label: t.riskUnknown, tone: "unknown" };
  }
}

export function burnText(ratePerHour: number | null, window: WindowType, t: Dict): string | null {
  if (ratePerHour === null) return null;
  return window === "weekly" ? t.perDay((ratePerHour * 24).toFixed(1)) : t.perHour(ratePerHour.toFixed(1));
}

export type ForecastMode = "insufficient" | "no_projection" | "unconfirmed" | "full";

export function forecastMode(f: UsageForecast): ForecastMode {
  switch (f.status) {
    case "insufficient_data":
      return "insufficient";
    case "not_projected":
      return "no_projection";
    case "reset_schedule_unconfirmed":
      return "unconfirmed";
    default:
      return "full";
  }
}

export function confidenceLabel(c: string, t: Dict): string {
  switch (c.toLowerCase()) {
    case "high":
      return t.confHigh;
    case "medium":
      return t.confMedium;
    default:
      return t.confLow;
  }
}

export function windowName(w: string | null, t: Dict): string {
  if (w === "five_hour") return t.fiveHourShort;
  if (w === "weekly") return t.weeklyShort;
  // Limits named by the source itself (Claude): "weekly_model:<name>", "statusline:<key>".
  if (w?.startsWith("weekly_model:")) return t.extraLimitWeekly(w.slice("weekly_model:".length));
  if (w?.startsWith("statusline:")) return w.slice("statusline:".length);
  return t.account;
}

/** "ChatGPT Plus" for Codex, "Claude Max" for Claude. */
export function planLabel(provider: string, planType: string, t: Dict): string {
  return provider === "claude" ? t.planClaude(planType) : t.plan(planType);
}

/** The plan beside the provider's name: "ChatGPT Plus", but "Max" where the name already says Claude. */
export function headPlan(provider: string, displayName: string, planType: string, t: Dict): string {
  const plan = planLabel(provider, planType, t);
  return plan.startsWith(`${displayName} `) ? plan.slice(displayName.length + 1) : plan;
}

/** "3 min ago" / "2 h ago". */
export function agoText(ms: number, now: number, t: Dict): string {
  const m = Math.max(0, Math.floor((now - ms) / 60_000));
  return m < 120 ? t.minutesAgo(m) : t.hoursAgo(Math.floor(m / 60));
}

/** User-facing title; never the raw enum, never a guessed cause. */
export function eventTitle(e: EventView, t: Dict): string {
  if (e.eventType === "natural_rollover_observed") {
    if (e.windowType === "five_hour") return t.ev_natural_rollover_observed_five_hour;
    if (e.windowType === "weekly") return t.ev_natural_rollover_observed_weekly;
    return t.ev_natural_rollover_observed;
  }
  if (e.eventType === "quota_refreshed_observed") {
    if (e.windowType === "five_hour") return t.ev_quota_refreshed_observed_five_hour;
    if (e.windowType === "weekly") return t.ev_quota_refreshed_observed_weekly;
    return t.ev_quota_refreshed_observed;
  }
  if (e.visibility === "user") return lookup(t, `ev_${e.eventType}`, e.eventType);
  return lookup(t, `diag_${e.eventType}`, e.eventType);
}

/** Diagnostics keep the technical enum name next to the translation (matches logs and DB). */
export function technicalName(eventType: string): string {
  return eventType
    .split("_")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join("");
}

function num(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

export function beforeAfter(e: EventView, t: Dict, lang: Lang): { before: string[]; after: string[] } | null {
  const ev = e.evidence;
  // Codex evidence uses usedPercentBefore/resetAtBefore; Claude uses usedBefore/previousResetsAtMs.
  const ub = num(ev.usedPercentBefore ?? ev.usedBefore);
  const ua = num(ev.usedPercentAfter ?? ev.usedAfter);
  if (ub === null && ua === null) return null;
  const rb = num(ev.resetAtBefore ?? ev.previousResetsAtMs);
  const ra = num(ev.resetAtAfter ?? ev.newResetsAtMs);
  const before = [ub === null ? "—" : t.usedValue(Math.round(ub))];
  if (rb !== null) before.push(t.resetAtValue(formatDateTime(rb, lang)));
  const after = [ua === null ? "—" : t.usedValue(Math.round(ua))];
  if (ev.slidingAnchorDetected === true) after.push(t.resetUnanchored);
  else if (ra !== null) after.push(t.resetAtValue(formatDateTime(ra, lang)));
  return { before, after };
}

export function evidenceLines(e: EventView, t: Dict): string[] {
  const ev = e.evidence;
  const out: string[] = [];
  if (ev.crossedPreviousResetTime === true) out.push(t.evCrossed);
  const ub = num(ev.usedPercentBefore ?? ev.usedBefore);
  const ua = num(ev.usedPercentAfter ?? ev.usedAfter);
  if (ub !== null && ua !== null) {
    if (ua < ub) out.push(t.evDropped(Math.round(ub - ua)));
    else if (ua > ub) out.push(t.evRose(Math.round(ua - ub)));
  }
  const cb = num(ev.creditsBefore ?? ev.creditBefore);
  const ca = num(ev.creditsAfter ?? ev.creditAfter);
  if (cb !== null && ca !== null) out.push(cb === ca ? t.evCreditUnchanged : t.evCreditChanged(cb, ca));
  const polls = num(ev.stableNewResetAtPolls);
  if (polls !== null && polls > 0) out.push(t.evStablePolls(polls));
  if (e.eventType === "unscheduled_quota_refresh" || e.eventType === "quota_window_changed" || ev.cause === "unknown") out.push(t.causeUnknown);
  return out;
}

/** "30 minutes" / "24 hours": the observation a forecast needs (the backend decides the value). */
export function spanText(ms: number, t: Dict): string {
  const minutes = Math.round(ms / 60_000);
  return minutes >= 60 && minutes % 60 === 0 ? t.spanHours(minutes / 60) : t.spanMinutes(minutes);
}
