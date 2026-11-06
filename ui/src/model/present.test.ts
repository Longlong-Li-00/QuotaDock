import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { EventView, ProviderTrayLine, TrayStatusModel, UsageForecast, WidgetReset, WindowStatus } from "../api/types.ts";
import { en } from "../i18n/en.ts";
import { zhCN } from "../i18n/zh-CN.ts";
import { agoText, availabilityPill, availabilityView, beforeAfter, eventTitle, evidenceLines, extraResetPhrase, forecastLine, forecastMode, freshnessView, headPlan, planLabel, resetPhrase, riskView, spanText, technicalName, toneLabel, topBanner, windowForecastLine, windowName, windowReset, windowTone, weekdayIsClear } from "./present.ts";

const NOW = Date.UTC(2026, 9, 8, 2, 0, 0);
const H = 3_600_000;

function w(over: Partial<WindowStatus>): WindowStatus {
  return {
    windowType: "five_hour",
    present: true,
    usedPercent: 12,
    remainingPercent: 88,
    reportedResetAtMs: NOW + 2 * H,
    confirmedResetAtMs: NOW + 2 * H,
    resetSchedule: "confirmed",
    observationState: "active",
    candidateState: "none",
    slidingAnchor: false,
    snapshotQuality: "normal",
    currentCycle: null,
    resetPrecisionMs: null,
    ...over,
  };
}

describe("reset phrase", () => {
  // 2026-11-03 is a Tuesday; all of these are local time, as the formatter is.
  const now = new Date(2026, 10, 3, 14, 7).getTime();
  const MIN = 60_000;
  const confirmed = (atMs: number, precisionMs: number | null = null): WidgetReset => ({ schedule: "confirmed", atMs, precisionMs });
  const none = (schedule: WidgetReset["schedule"]): WidgetReset => ({ schedule, atMs: null, precisionMs: null });
  const phrases = (r: WidgetReset) => [resetPhrase(r, "overview", now, zhCN, "zh-CN"), resetPhrase(r, "overview", now, en, "en"), resetPhrase(r, "widget", now, zhCN, "zh-CN"), resetPhrase(r, "widget", now, en, "en")];

  const rows: [string, WidgetReset, [string, string, string, string]][] = [
    ["confirmed exact, under 24 h", confirmed(now + 2 * H + 13 * MIN), ["2 小时 13 分后重置", "Resets in 2 h 13 min", "2小时13分后", "in 2h 13m"]],
    ["confirmed exact, under an hour", confirmed(now + 47 * MIN), ["47 分钟后重置", "Resets in 47 min", "47分钟后", "in 47m"]],
    ["confirmed exact, whole hours", confirmed(now + 3 * H), ["3 小时后重置", "Resets in 3 h", "3小时后", "in 3h"]],
    ["confirmed exact, 24 h to 7 days", confirmed(new Date(2026, 10, 5, 16, 20).getTime()), ["周四 16:20 重置", "Resets Thu 16:20", "周四 16:20", "Thu 16:20"]],
    ["confirmed exact, 7 days or more", confirmed(new Date(2026, 10, 12, 16, 20).getTime()), ["11月12日 16:20 重置", "Resets Nov 12, 16:20", "11月12日", "Nov 12"]],
    ["approximate, today", confirmed(new Date(2026, 10, 3, 14, 30).getTime(), 60_000), ["预计 14:30 重置", "Expected to reset 14:30", "预计 14:30", "~14:30"]],
    ["approximate, not today", confirmed(new Date(2026, 10, 5, 14, 30).getTime(), 60_000), ["预计 周四 14:30 重置", "Expected to reset Thu 14:30", "预计 周四 14:30", "~Thu 14:30"]],
    ["approximate, 7 days or more", confirmed(new Date(2026, 10, 12, 14, 30).getTime(), 60_000), ["预计 11月12日 14:30 重置", "Expected to reset Nov 12, 14:30", "预计 11月12日", "~Nov 12"]],
    ["sliding", none("sliding"), ["空闲 · 首次使用后开始计时", "Idle · Timer starts on first use", "首次使用后计时", "Starts on first use"]],
    ["unconfirmed", none("unconfirmed"), ["重置时间待确认", "Reset time not confirmed", "待确认", "Unconfirmed"]],
    ["passed", none("passed"), ["已到重置时间，等待下一次更新", "Reset time passed, waiting for an update", "等待更新", "Waiting"]],
    ["window missing", none("missing"), ["最新数据中未报告", "Not reported in the latest data", "未报告", "Not reported"]],
  ];
  for (const [name, reset, expected] of rows) {
    it(name, () => assert.deepEqual(phrases(reset), expected));
  }

  it("24 hours exactly is a weekday, 7 days exactly is a date", () => {
    assert.equal(resetPhrase(confirmed(now + 24 * H), "overview", now, en, "en"), "Resets Wed 14:07");
    assert.equal(resetPhrase(confirmed(now + 24 * H - 1), "overview", now, en, "en"), "Resets in 23 h 59 min");
    assert.equal(resetPhrase(confirmed(now + 7 * 24 * H), "widget", now, en, "en"), "Nov 10");
    assert.equal(resetPhrase(confirmed(now + 7 * 24 * H - 1), "widget", now, en, "en"), "Nov 10", "today's own weekday would read as already gone");
  });

  it("on the weekly reset day the next reset takes its date, not today's weekday", () => {
    const at = new Date(2026, 10, 10, 15, 0).getTime();
    const tue16 = new Date(2026, 10, 3, 16, 0).getTime();
    const all = (r: WidgetReset) => [resetPhrase(r, "overview", tue16, zhCN, "zh-CN"), resetPhrase(r, "overview", tue16, en, "en"), resetPhrase(r, "widget", tue16, zhCN, "zh-CN"), resetPhrase(r, "widget", tue16, en, "en")];
    assert.deepEqual(all(confirmed(at)), ["11月10日 15:00 重置", "Resets Nov 10, 15:00", "11月10日", "Nov 10"]);
    assert.deepEqual(all(confirmed(at, 60_000)), ["预计 11月10日 15:00 重置", "Expected to reset Nov 10, 15:00", "预计 11月10日", "~Nov 10"]);
    assert.equal(weekdayIsClear(at, tue16), false);
    assert.equal(weekdayIsClear(new Date(2026, 10, 9, 15, 0).getTime(), tue16), true, "Monday is the coming Monday");
    assert.equal(weekdayIsClear(new Date(2026, 10, 11, 15, 0).getTime(), tue16), false, "a week or more out needs its date anyway");
  });

  it("midnight reads 00:05 in both languages", () => {
    const at = new Date(2026, 10, 5, 0, 5).getTime();
    assert.equal(resetPhrase(confirmed(at), "widget", now, zhCN, "zh-CN"), "周四 00:05");
    assert.equal(resetPhrase(confirmed(at), "widget", now, en, "en"), "Thu 00:05");
  });

  it("the status line's 1000 ms precision is exact; only a coarser one is approximate", () => {
    assert.equal(resetPhrase(confirmed(now + 2 * H, 1000), "overview", now, en, "en"), "Resets in 2 h");
    assert.equal(resetPhrase(confirmed(now + 2 * H, 1001), "overview", now, en, "en"), "Expected to reset 16:07");
  });

  it("the last minute still counts down, a passed time never does", () => {
    assert.equal(resetPhrase(confirmed(now + 30_000), "overview", now, en, "en"), "Resets in 1 min");
    assert.deepEqual(phrases(confirmed(now)), phrases(none("passed")));
    assert.deepEqual(phrases(confirmed(now - MIN)), phrases(none("passed")));
    assert.deepEqual(phrases(confirmed(now - MIN, 60_000)), phrases(none("passed")));
  });

  it("a confirmed schedule without a time claims no time", () => {
    assert.deepEqual(phrases(none("confirmed")), phrases(none("unconfirmed")));
  });

  it("sliding, unconfirmed and passed never show a time, even when a raw reset time exists", () => {
    const raw = w({ reportedResetAtMs: now + 5 * H, confirmedResetAtMs: null });
    for (const schedule of ["sliding", "unconfirmed", "passed"] as const) {
      const reset = windowReset({ ...raw, resetSchedule: schedule });
      assert.equal(reset.atMs, null);
      for (const text of phrases(reset)) assert.doesNotMatch(text, /\d/);
    }
    // Even a time that reached the formatter by mistake stays unused.
    for (const text of phrases({ schedule: "sliding", atMs: now + 5 * H, precisionMs: null })) assert.doesNotMatch(text, /\d/);
  });

  it("reads a window's confirmed time and precision, and a missing window as missing", () => {
    assert.deepEqual(windowReset(w({ confirmedResetAtMs: now + H, resetPrecisionMs: 60_000 })), { schedule: "confirmed", atMs: now + H, precisionMs: 60_000 });
    assert.equal(windowReset(w({ present: false, usedPercent: null, remainingPercent: null })).schedule, "missing");
  });
});

describe("window tone", () => {
  it("is the backend's; the UI applies no thresholds", () => {
    assert.equal(windowTone(w({ remainingPercent: 4 }), "normal"), "normal");
    assert.equal(windowTone(w({ remainingPercent: 88 }), "critical"), "critical");
    assert.equal(windowTone(w({}), undefined), "unknown");
  });
  it("a passed reset stays neutral", () => {
    assert.equal(windowTone(w({ resetSchedule: "passed", remainingPercent: 4 }), "critical"), "muted");
  });
  it("low tones carry words, the others none", () => {
    assert.equal(toneLabel("low", zhCN), "余量低");
    assert.equal(toneLabel("critical", zhCN), "余量极低");
    assert.equal(toneLabel("low", en), "Low");
    assert.equal(toneLabel("critical", en), "Very low");
    for (const tone of ["normal", "muted", "unknown"] as const) assert.equal(toneLabel(tone, en), null);
  });
});

describe("forecast line", () => {
  const now = new Date(2026, 10, 3, 14, 7).getTime();
  const f = (over: Partial<UsageForecast>): UsageForecast =>
    ({ windowType: "weekly", status: "ok", risk: "at_risk", remainingPercent: 22, estimatedExhaustionAtMs: new Date(2026, 10, 5, 9, 0).getTime(), serverResetAtMs: new Date(2026, 10, 7, 9, 0).getTime(), ...over }) as UsageForecast;

  it("at risk with a date names the day and keeps the word estimated", () => {
    assert.equal(forecastLine(f({}), now, zhCN, "zh-CN"), "按当前速度约 周四 用完，早于重置（估算）");
    assert.equal(forecastLine(f({}), now, en, "en"), "May run out around Thu, before reset (estimated)");
  });
  it("uses the same day and date rule as the reset line", () => {
    assert.equal(forecastLine(f({ estimatedExhaustionAtMs: new Date(2026, 10, 3, 18, 30).getTime() }), now, zhCN, "zh-CN"), "按当前速度约 18:30 用完，早于重置（估算）");
    assert.equal(forecastLine(f({ estimatedExhaustionAtMs: new Date(2026, 10, 12, 9, 0).getTime() }), now, zhCN, "zh-CN"), "按当前速度约 11月12日 用完，早于重置（估算）");
    assert.equal(forecastLine(f({ estimatedExhaustionAtMs: new Date(2026, 10, 12, 9, 0).getTime() }), now, en, "en"), "May run out around Nov 12, before reset (estimated)");
  });
  it("at risk without a usable date says so without one", () => {
    for (const at of [null, now - 1]) {
      assert.equal(forecastLine(f({ estimatedExhaustionAtMs: at }), now, zhCN, "zh-CN"), "按当前速度可能在重置前用完（估算）");
      assert.equal(forecastLine(f({ estimatedExhaustionAtMs: at }), now, en, "en"), "May run out before reset at this rate (estimated)");
    }
  });
  it("a date on today's own weekday takes the date form, as the reset line does", () => {
    const nextTue = new Date(2026, 10, 10, 9, 0).getTime();
    assert.equal(forecastLine(f({ estimatedExhaustionAtMs: nextTue, serverResetAtMs: nextTue + 24 * H }), now, zhCN, "zh-CN"), "按当前速度约 11月10日 用完，早于重置（估算）");
    assert.equal(forecastLine(f({ estimatedExhaustionAtMs: nextTue, serverResetAtMs: nextTue + 24 * H }), now, en, "en"), "May run out around Nov 10, before reset (estimated)");
    assert.equal(forecastLine(f({ estimatedExhaustionAtMs: new Date(2026, 10, 9, 9, 0).getTime(), serverResetAtMs: nextTue }), now, en, "en"), "May run out around Mon, before reset (estimated)");
  });
  it("a reset that has gone by takes the line away, past or future estimate alike", () => {
    for (const serverResetAtMs of [now, now - H]) {
      for (const at of [null, now - 1, now + H]) {
        assert.equal(forecastLine(f({ serverResetAtMs, estimatedExhaustionAtMs: at }), now, zhCN, "zh-CN"), null);
        assert.equal(forecastLine(f({ serverResetAtMs, estimatedExhaustionAtMs: at }), now, en, "en"), null);
      }
    }
  });
  it("a future reset with an estimate already past keeps the undated sentence", () => {
    assert.equal(forecastLine(f({ serverResetAtMs: now + 40 * H, estimatedExhaustionAtMs: now - H }), now, zhCN, "zh-CN"), "按当前速度可能在重置前用完（估算）");
    assert.equal(forecastLine(f({ serverResetAtMs: now + 40 * H, estimatedExhaustionAtMs: now - H }), now, en, "en"), "May run out before reset at this rate (estimated)");
  });
  it("an unknown reset does not take the line away by itself", () => {
    assert.equal(forecastLine(f({ serverResetAtMs: null }), now, en, "en"), "May run out around Thu, before reset (estimated)");
  });
  it("no result never reads as safe: everything else shows nothing", () => {
    for (const risk of ["watch", "low", "insufficient_data"] as const) assert.equal(forecastLine(f({ risk }), now, en, "en"), null);
    assert.equal(forecastLine(f({ status: "reset_schedule_unconfirmed" }), now, en, "en"), null);
    assert.equal(forecastLine(f({ remainingPercent: 0 }), now, en, "en"), null);
    assert.equal(forecastLine(f({ remainingPercent: null }), now, en, "en"), null);
  });
});

describe("forecast line under a window's reset line", () => {
  const now = new Date(2026, 10, 3, 14, 7).getTime();
  // The forecast's own fields say nothing is wrong: each case is decided by the window alone.
  const f = { windowType: "weekly", status: "ok", risk: "at_risk", remainingPercent: 22, estimatedExhaustionAtMs: now + 40 * H, serverResetAtMs: now + 80 * H } as UsageForecast;
  const weekly = (over: Partial<WindowStatus>) => w({ windowType: "weekly", confirmedResetAtMs: now + 80 * H, ...over });

  it("stays while the reset line names a coming reset", () => {
    assert.equal(windowForecastLine(weekly({}), f, now, zhCN, "zh-CN"), "按当前速度约 周四 用完，早于重置（估算）");
    assert.equal(windowForecastLine(weekly({}), f, now, en, "en"), "May run out around Thu, before reset (estimated)");
  });
  const none: [string, Partial<WindowStatus>, string, string][] = [
    ["missing window", { present: false, usedPercent: null, remainingPercent: null }, "最新数据中未报告", "Not reported in the latest data"],
    ["passed schedule", { resetSchedule: "passed", confirmedResetAtMs: null }, "已到重置时间，等待下一次更新", "Reset time passed, waiting for an update"],
    ["confirmed time gone by between polls", { confirmedResetAtMs: now - H }, "已到重置时间，等待下一次更新", "Reset time passed, waiting for an update"],
    ["confirmed time exactly now", { confirmedResetAtMs: now }, "已到重置时间，等待下一次更新", "Reset time passed, waiting for an update"],
  ];
  for (const [name, over, zh, english] of none) {
    it(`none beside a ${name}, in either language`, () => {
      const win = weekly(over);
      assert.equal(resetPhrase(windowReset(win), "overview", now, zhCN, "zh-CN"), zh);
      assert.equal(resetPhrase(windowReset(win), "overview", now, en, "en"), english);
      assert.equal(windowForecastLine(win, f, now, zhCN, "zh-CN"), null);
      assert.equal(windowForecastLine(win, f, now, en, "en"), null);
    });
  }
});

describe("extra limit reset", () => {
  const now = new Date(2026, 10, 3, 14, 7).getTime();
  const thu = new Date(2026, 10, 5, 16, 20).getTime();
  it("a coarse time is expected, in either language", () => {
    assert.equal(extraResetPhrase(thu, 60_000, now, zhCN, "zh-CN"), "预计 周四 16:20 重置");
    assert.equal(extraResetPhrase(thu, 60_000, now, en, "en"), "Expected to reset Thu 16:20");
  });
  it("a time to the second, or of unknown precision, is stated", () => {
    for (const precision of [1000, null]) {
      assert.equal(extraResetPhrase(thu, precision, now, zhCN, "zh-CN"), "周四 16:20 重置");
      assert.equal(extraResetPhrase(thu, precision, now, en, "en"), "Resets Thu 16:20");
    }
  });
  it("is never a countdown, however near", () => {
    const soon = now + 2 * H;
    assert.equal(extraResetPhrase(soon, 1000, now, en, "en"), "Resets 16:07");
    assert.equal(extraResetPhrase(soon, 60_000, now, en, "en"), "Expected to reset 16:07");
  });
});

describe("top banner", () => {
  const line = (provider: ProviderTrayLine["provider"], displayName: string, reasons: string[]): ProviderTrayLine => ({
    provider,
    displayName,
    severity: "normal",
    reasons,
    fiveHourRemaining: null,
    weeklyRemaining: null,
    fiveHourTone: "unknown",
    weeklyTone: "unknown",
    live: true,
    hasData: true,
    note: null,
  });
  const tray = (...providers: ProviderTrayLine[]) => ({ providers }) as TrayStatusModel;

  it("quota low and forecast risk alone get no banner", () => {
    assert.equal(topBanner(tray(line("codex", "Codex", ["weekly_remaining_30", "five_hour_remaining_10", "weekly_forecast_at_risk"]), line("claude", "Claude", ["weekly_remaining_10"])), en), null);
    assert.equal(topBanner(tray(line("codex", "Codex", [])), en), null);
  });
  it("names the provider unless the sentence already does", () => {
    assert.deepEqual(topBanner(tray(line("codex", "Codex", ["limit_reached", "weekly_remaining_30"])), en), { text: "Codex: Limit reached", tone: "critical" });
    assert.deepEqual(topBanner(tray(line("codex", "Codex", ["limit_reached", "weekly_remaining_30"])), zhCN), { text: "Codex：已达到限制", tone: "critical" }, "the full-width colon of the other Chinese strings");
    assert.deepEqual(topBanner(tray(line("codex", "Codex", ["not_authenticated"])), zhCN), { text: "Codex 未登录", tone: "critical" });
  });
  it("persistent unknown availability is a warning; an error outranks it", () => {
    assert.equal(topBanner(tray(line("codex", "Codex", ["availability_unknown_persistent"])), en)?.tone, "warning");
    const covered = new Set(["codex"]);
    assert.equal(topBanner(tray(line("codex", "Codex", ["not_authenticated"])), en, covered), null, "the card's own banner says it");
    assert.deepEqual(topBanner(tray(line("codex", "Codex", ["not_authenticated", "limit_reached"])), en, covered), { text: "Codex: Limit reached", tone: "critical" }, "a limit stays above the cards");
    assert.equal(topBanner(tray(line("codex", "Codex", []), line("claude", "Claude", ["not_authenticated"])), en, covered)?.text.includes("Claude"), true, "only the covered provider is dropped");
    assert.equal(topBanner(tray(line("codex", "Codex", ["availability_unknown_persistent", "no_valid_snapshot_30m"])), en)?.tone, "critical");
  });
});

describe("availability presentation", () => {
  it("maps only the backend flag, independent of percentages", () => {
    assert.deepEqual(availabilityView(true, false, en), { label: "Available", tone: "ok" });
    assert.deepEqual(availabilityView(false, false, en), { label: "Limit reached", tone: "critical" });
    assert.deepEqual(availabilityView(null, false, en), { label: "Availability unknown", tone: "unknown" });
    assert.equal(availabilityView(null, true, en).label, "Availability has remained unknown");
    assert.equal(availabilityView(true, false, zhCN).label, "可用");
  });
  it("the card's pill is only for a Codex limit or availability that stays unknown", () => {
    assert.equal(availabilityPill("codex", true, false, en), null);
    assert.equal(availabilityPill("codex", null, false, en), null);
    assert.deepEqual(availabilityPill("codex", false, false, en), { label: "Limit reached", tone: "critical" });
    assert.deepEqual(availabilityPill("codex", null, true, en), { label: "Availability has remained unknown", tone: "warning" });
  });
  it("Claude never gets an availability claim", () => {
    for (const allowed of [true, false, null]) assert.equal(availabilityPill("claude", allowed, true, en), null);
  });
});

describe("freshness", () => {
  const tray = (over: Partial<TrayStatusModel>): TrayStatusModel => ({
    providers: [],
    severity: "normal",
    icon: "normal",
    paused: false,
    availability: true,
    availabilityUnknownPersistent: false,
    fiveHourRemaining: 88,
    weeklyRemaining: 98,
    updatedAtMs: NOW - 42_000,
    freshness: "fresh",
    minutesSinceUpdate: 0,
    reasons: [],
    tooltipLines: [],
    ...over,
  });
  it("uses the core's freshness class and only formats the age", () => {
    assert.equal(freshnessView(tray({}), NOW, en).text, "just now");
    assert.equal(freshnessView(tray({ updatedAtMs: NOW - 3 * 60_000 }), NOW, zhCN).text, "3 分钟前");
    assert.match(freshnessView(tray({ updatedAtMs: NOW - 12 * 60_000, freshness: "maybe_stale" }), NOW, en).text, /Data may be stale/);
    assert.equal(freshnessView(tray({ updatedAtMs: NOW - 34 * 60_000, freshness: "stale" }), NOW, en).tone, "critical");
    assert.equal(freshnessView(tray({ updatedAtMs: null, freshness: "no_data" }), NOW, en).text, "No data yet");
  });
  it("an old age while paused is expected, not a warning", () => {
    assert.deepEqual(freshnessView(tray({ paused: true, updatedAtMs: NOW - 34 * 60_000, freshness: "stale" }), NOW, en), { text: "34 min ago", tone: "neutral" });
  });
});

describe("forecast and risk wording", () => {
  it("risk labels are hedged, never Safe/Danger", () => {
    assert.equal(riskView("at_risk", en).label, "May run out early");
    assert.equal(riskView("at_risk", zhCN).label, "可能提前用完");
    assert.equal(riskView("watch", zhCN).label, "关注");
    assert.equal(riskView("insufficient_data", en).label, "—");
  });
  it("maps forecast status without inventing values", () => {
    const f = { status: "insufficient_data" } as UsageForecast;
    assert.equal(forecastMode(f), "insufficient");
    assert.equal(forecastMode({ ...f, status: "not_projected" }), "no_projection");
    assert.equal(forecastMode({ ...f, status: "reset_schedule_unconfirmed" }), "unconfirmed");
  });
});

describe("event wording", () => {
  const ev = (over: Partial<EventView>): EventView => ({
    id: 1,
    provider: "codex",
    eventKey: "k",
    windowType: "five_hour",
    eventType: "natural_rollover_observed",
    visibility: "user",
    observedAtMs: NOW,
    confidence: "medium",
    confirmed: true,
    evidence: {},
    supersededByEventKey: null,
    ...over,
  });
  it("never says 'new window started'", () => {
    const title = eventTitle(ev({}), en);
    assert.equal(title, "5-hour quota reset");
    assert.doesNotMatch(title, /new window|started/i);
    assert.equal(eventTitle(ev({}), zhCN), "5 小时额度已重置");
  });
  it("unscheduled refresh states an unknown cause and never a promotion", () => {
    const e = ev({ eventType: "unscheduled_quota_refresh", windowType: "weekly", evidence: { usedPercentBefore: 70, usedPercentAfter: 1, creditsBefore: 2, creditsAfter: 2, cause: "unknown" } });
    assert.equal(eventTitle(e, en), "Quota reset early");
    const lines = evidenceLines(e, en);
    assert.ok(lines.includes("Cause unknown"));
    assert.ok(lines.includes("Usage dropped 69 pp"));
    assert.ok(lines.includes("Credit count unchanged"));
    assert.ok(!lines.join(" ").match(/promot|global/i));
  });
  it("diagnostics are translated and keep their technical name available", () => {
    const e = ev({ eventType: "transient_backend_state", visibility: "diagnostic" });
    assert.equal(eventTitle(e, zhCN), "后端返回了短暂且随后恢复的额度状态");
    assert.equal(technicalName(e.eventType), "TransientBackendState");
  });
});

describe("dictionaries", () => {
  it("zh-CN defines exactly the English keys", () => {
    assert.deepEqual(Object.keys(zhCN).sort(), Object.keys(en).sort());
  });
});

describe("claude presentation", () => {
  const ev = (over: Partial<EventView>): EventView => ({
    id: -1,
    provider: "claude",
    eventKey: "c",
    windowType: "five_hour",
    eventType: "quota_refreshed_observed",
    visibility: "user",
    observedAtMs: 0,
    confidence: "medium",
    confirmed: true,
    evidence: { usedBefore: 81, usedAfter: 3, previousResetsAtMs: 1_000, newResetsAtMs: 2_000 },
    supersededByEventKey: null,
    ...over,
  });
  it("names Claude refreshes by window and never claims a cause", () => {
    assert.equal(eventTitle(ev({}), en), "5-hour quota reset");
    assert.equal(eventTitle(ev({ windowType: "weekly" }), zhCN), "每周额度已重置");
    assert.equal(eventTitle(ev({ visibility: "diagnostic", eventType: "limit_reached_observed" }), en), "Usage reached 100% (availability not inferred)");
    assert.ok(evidenceLines(ev({ visibility: "diagnostic", eventType: "quota_window_changed" }), en).includes("Cause unknown"));
  });
  it("reads Claude evidence for before/after", () => {
    const ba = beforeAfter(ev({}), en, "en");
    assert.equal(ba?.before[0], "81% used");
    assert.equal(ba?.after[0], "3% used");
  });
  it("labels plans and source-named limits per provider", () => {
    assert.equal(planLabel("claude", "max", en), "Claude Max");
    assert.equal(planLabel("codex", "plus", en), "ChatGPT Plus");
    assert.equal(headPlan("codex", "Codex", "plus", en), "ChatGPT Plus");
    assert.equal(headPlan("claude", "Claude", "max", en), "Max");
    assert.equal(windowName("weekly_model:fable", en), "fable (weekly)");
    assert.equal(agoText(0, 3 * 60_000, en), "3 min ago");
    assert.equal(agoText(0, 3 * 3_600_000, zhCN), "3 小时前");
  });
});

describe("forecast history requirement", () => {
  it("states the span the backend requires", () => {
    assert.equal(en.notEnoughHelp(spanText(30 * 60_000, en)), "At least 30 minutes of observation and measurable usage are required.");
    assert.equal(spanText(24 * 3_600_000, en), "24 hours");
    assert.equal(zhCN.notEnoughHelp(spanText(24 * 3_600_000, zhCN)), "至少需要 24 小时的观察和可测量的用量。");
  });
});
