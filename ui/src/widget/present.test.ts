import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { WidgetProvider, WidgetView } from "../api/types.ts";
import { en } from "../i18n/en.ts";
import { zhCN } from "../i18n/zh-CN.ts";
import { formatDay, formatTime } from "../model/present.ts";
import { ageText, barWidth, headerStatus, remainingText, ringView, rowView } from "./present.ts";

// Local time, as the formatters are: 14:00, so a reset 2 h out is today in every time zone.
const NOW = new Date(2026, 9, 11, 14, 0, 0).getTime();
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

function p(over: Partial<WidgetProvider>): WidgetProvider {
  return {
    id: "codex",
    displayName: "Codex",
    shortLabel: "C",
    fiveHourRemaining: 88,
    weeklyRemaining: 82,
    fiveHourTone: "normal",
    weeklyTone: "normal",
    fiveHourReset: { schedule: "confirmed", atMs: NOW + 2 * 60 * MIN, precisionMs: null },
    weeklyReset: { schedule: "confirmed", atMs: NOW + 3 * 24 * 60 * MIN, precisionMs: null },
    showFiveHour: true,
    state: "live",
    staleKind: null,
    availability: "allowed",
    severity: "normal",
    updatedAtMs: NOW - 30_000,
    note: null,
    noteShort: null,
    waitingNote: null,
    ...over,
  };
}

function view(over: Partial<WidgetView>): WidgetView {
  return {
    generatedAtMs: NOW,
    paused: false,
    globalSeverity: "normal",
    language: "en",
    theme: "system",
    mode: "compact",
    opacity: 85,
    locked: false,
    providers: [p({})],
    updatedAtMs: NOW - 30_000,
    customIcons: {},
    ...over,
  };
}

describe("widget values", () => {
  it("shows remaining percent, never used", () => {
    const r = rowView(p({ fiveHourRemaining: 88.4, weeklyRemaining: 17.6 }), NOW, en, "en");
    assert.equal(r.five, "88%");
    assert.equal(r.weekly, "18%");
    assert.ok(!r.label.includes("used"));
    assert.match(r.label, /88% left/);
  });

  it("missing data shows a dash and Waiting / 等待数据", () => {
    const missing = p({ id: "claude", displayName: "Claude", fiveHourRemaining: null, weeklyRemaining: null, fiveHourTone: "unknown", weeklyTone: "unknown", state: "waiting", availability: "unknown", updatedAtMs: null });
    const r = rowView(missing, NOW, en, "en");
    assert.deepEqual([r.five, r.weekly, r.status, r.fiveTone, r.weeklyTone], ["—", "—", "Waiting", "unknown", "unknown"]);
    assert.equal(rowView(missing, NOW, zhCN, "zh-CN").status, "等待数据");
    assert.equal(barWidth(null), 0);
  });

  it("a row with no data says why when the backend names a cause, and stays quiet", () => {
    const noData = { id: "claude", displayName: "Claude", fiveHourRemaining: null, weeklyRemaining: null, fiveHourTone: "unknown", weeklyTone: "unknown", state: "waiting", availability: "unknown", updatedAtMs: null } as const;
    const causes = [
      ["Not set up", "未设置"],
      ["API billing", "API 计费"],
      ["Not installed", "未安装"],
      ["Not signed in", "未登录"],
    ] as const;
    for (const [word, zh] of causes) {
      const e = rowView(p({ ...noData, note: word, waitingNote: word }), NOW, en, "en");
      assert.deepEqual([e.status, e.statusTone, e.note?.text, e.note?.tone, e.wide, e.five, e.weekly], [word, "muted", word, "muted", false, "—", "—"]);
      assert.match(e.label, new RegExp(`\\(${word}\\)`), "the tooltip follows");
      assert.equal(e.label.includes("Waiting"), false);
      assert.equal(rowView(p({ ...noData, note: zh, waitingNote: zh }), NOW, zhCN, "zh-CN").status, zh);
    }
    const trueWait = rowView(p({ ...noData, note: "Waiting for Claude Code" }), NOW, en, "en");
    assert.deepEqual([trueWait.status, trueWait.note?.text], ["Waiting", "Waiting"], "the backend's wait keeps the widget's word");
    assert.equal(trueWait.label.includes("Claude Code"), false);
  });

  it("a waiting row takes the note's short form in the compact slot, the whole words elsewhere", () => {
    const noData = { id: "claude", displayName: "Claude", fiveHourRemaining: null, weeklyRemaining: null, fiveHourTone: "unknown", weeklyTone: "unknown", state: "waiting", availability: "unknown", updatedAtMs: null } as const;
    const r = rowView(p({ ...noData, note: "Needs verification", noteShort: "Needs check", waitingNote: "Needs verification" }), NOW, en, "en");
    assert.deepEqual([r.status, r.note?.text, r.note?.tone], ["Needs verification", "Needs check", "muted"]);
    assert.match(r.label, /\(Needs verification\)/);
    const read = rowView(p({ ...noData, note: "Cannot read data", noteShort: "Can't read", waitingNote: "Cannot read data", showFiveHour: false }), NOW, en, "en");
    assert.deepEqual([read.status, read.note?.text], ["Cannot read data", "Can't read"], "with the 5-hour column hidden too");
    const zh = rowView(p({ ...noData, note: "需要验证", waitingNote: "需要验证" }), NOW, zhCN, "zh-CN");
    assert.deepEqual([zh.status, zh.note?.text], ["需要验证", "需要验证"], "no short form: the words fit");
  });

  it("Codex polls failing with nothing stored: the cannot-refresh word, muted, with its dashes", () => {
    const failing = { fiveHourRemaining: null, weeklyRemaining: null, fiveHourTone: "unknown", weeklyTone: "unknown", state: "waiting", availability: "unknown", updatedAtMs: null } as const;
    const e = rowView(p({ ...failing, waitingNote: "Can't refresh" }), NOW, en, "en");
    assert.deepEqual([e.status, e.statusTone, e.wide, e.note?.text, e.five, e.weekly], ["Can't refresh", "muted", false, "Can't refresh", "—", "—"]);
    assert.match(e.label, /\(Can't refresh\)/);
    assert.deepEqual(rowView(p({ ...failing, waitingNote: "无法刷新" }), NOW, zhCN, "zh-CN").note, { text: "无法刷新", tone: "muted" });
    assert.equal(rowView(p({ ...failing }), NOW, en, "en").status, "Waiting", "first launch: the true wait");
  });

  it("a backend note on a row with current data changes nothing", () => {
    const r = rowView(p({ id: "claude", displayName: "Claude", availability: "unknown", note: "Not signed in", noteShort: "Sign in" }), NOW, en, "en");
    assert.deepEqual([r.status, r.note], [null, null]);
  });

  it("stale data keeps the last values and says when they are from", () => {
    const at = NOW - 47 * MIN;
    const stale = { state: "stale", staleKind: "stale" } as const;
    const r = rowView(p({ ...stale, updatedAtMs: at }), NOW, en, "en");
    const clock = formatTime(at, "en");
    assert.deepEqual([r.five, r.weekly, r.status, r.statusTone], ["88%", "82%", `Stale · ${clock}`, "critical"]);
    assert.equal(rowView(p({ ...stale, updatedAtMs: at }), NOW, zhCN, "zh-CN").status, `数据已过时 · ${formatTime(at, "zh-CN")}`);
    const older = NOW - 3 * DAY;
    assert.equal(rowView(p({ ...stale, updatedAtMs: older }), NOW, zhCN, "zh-CN").status, `数据已过时 · ${formatDay(older, "zh-CN")}`, "not today: the date, not a time that looks like today");
  });

  it("may be stale is amber and says so, with the short form in the compact slot", () => {
    const at = NOW - 14 * MIN;
    const maybe = { state: "stale", staleKind: "maybe", updatedAtMs: at } as const;
    const zh = rowView(p(maybe), NOW, zhCN, "zh-CN");
    assert.deepEqual([zh.status, zh.statusTone, zh.note?.text, zh.note?.tone], [`可能已过时 · ${formatTime(at, "zh-CN")}`, "warning", "可能已过时", "warning"]);
    const e = rowView(p(maybe), NOW, en, "en");
    assert.deepEqual([e.status, e.statusTone, e.note?.text], [`May be stale · ${formatTime(at, "en")}`, "warning", "May be stale"]);
    assert.equal(rowView(p({ ...maybe, showFiveHour: false }), NOW, en, "en").note?.text, `May be stale · ${formatTime(at, "en")}`, "no 5-hour column: room for the time");
    const older = NOW - DAY;
    assert.equal(rowView(p({ ...maybe, updatedAtMs: older }), NOW, zhCN, "zh-CN").status, `可能已过时 · ${formatDay(older, "zh-CN")}`);
  });

  it("not live is quiet, not amber and not an alarm: Claude's resting state", () => {
    const at = NOW - 47 * MIN;
    const notLive = { id: "claude", displayName: "Claude", availability: "unknown", state: "stale", staleKind: "not_live", updatedAtMs: at, note: "Not live" } as const;
    // The backend's note is in the view's language.
    const notLiveZh = { ...notLive, note: "非实时" } as const;
    const zh = rowView(p(notLiveZh), NOW, zhCN, "zh-CN");
    assert.deepEqual([zh.status, zh.statusTone, zh.note?.text, zh.note?.tone], [`非实时 · ${formatTime(at, "zh-CN")}`, "muted", `非实时 · ${formatTime(at, "zh-CN")}`, "muted"]);
    const e = rowView(p(notLive), NOW, en, "en");
    assert.deepEqual([e.status, e.statusTone, e.note?.text], [`Not live · ${formatTime(at, "en")}`, "muted", "Not live"], "with the time it does not fit beside two columns");
    assert.match(zh.label, /非实时/);
    const older = NOW - 3 * DAY;
    assert.equal(rowView(p({ ...notLiveZh, updatedAtMs: older }), NOW, zhCN, "zh-CN").status, `非实时 · ${formatDay(older, "zh-CN")}`);
    assert.equal(rowView(p({ ...notLiveZh, updatedAtMs: older }), NOW, zhCN, "zh-CN").note?.text, "非实时");
    // A named cause says the cause, as the tray does: Claude not set up with old data.
    const notSetUp = rowView(p({ ...notLiveZh, note: "未设置", noteShort: null }), NOW, zhCN, "zh-CN");
    assert.deepEqual([notSetUp.status, notSetUp.note?.text, notSetUp.statusTone], ["未设置", "未设置", "muted"]);
    const noTime = rowView(p({ ...notLive, updatedAtMs: null }), NOW, en, "en");
    assert.deepEqual([noTime.status, noTime.statusTone], ["Not live", "muted"], "the backend's own words when the time is not known");
  });

  it("old data with no values keeps its dashes; it never takes over the row", () => {
    const r = rowView(p({ state: "stale", staleKind: "stale", fiveHourRemaining: null, weeklyRemaining: null, updatedAtMs: NOW - 47 * MIN }), NOW, en, "en");
    assert.deepEqual([r.wide, r.statusTone, r.five, r.weekly], [false, "critical", "—", "—"]);
  });

  it("each value keeps its own tone from the backend, also when stale", () => {
    const r = rowView(p({ state: "stale", staleKind: "not_live", fiveHourRemaining: 82, weeklyRemaining: 22, fiveHourTone: "muted", weeklyTone: "low", updatedAtMs: NOW - 47 * MIN }), NOW, en, "en");
    assert.deepEqual([r.fiveTone, r.weeklyTone], ["muted", "low"]);
  });

  it("paused keeps the last values; the header says Paused / 已暂停", () => {
    const r = rowView(p({ state: "paused", weeklyRemaining: 22, weeklyTone: "low" }), NOW, en, "en");
    assert.deepEqual([r.five, r.status, r.fiveTone, r.weeklyTone, r.note?.tone], ["88%", null, "muted", "muted", "muted"]);
    assert.deepEqual(headerStatus(view({ paused: true }), NOW, en), { text: "Paused", tone: "warning" });
    assert.equal(ringView(p({ state: "paused" }), NOW, zhCN, "zh-CN").line.text, "已暂停", "the ring layout has no header: the row says it");
  });

  it("errors show the backend's note, else when the last data is from, else a generic word", () => {
    const noData = rowView(p({ state: "error", severity: "error", note: "Sign in to Codex first", fiveHourRemaining: null, weeklyRemaining: null, updatedAtMs: NOW - 47 * MIN }), NOW, en, "en");
    assert.deepEqual([noData.status, noData.wide, noData.note?.text], ["Sign in to Codex first", true, "Sign in to Codex first"], "no values: the message gets the row");
    assert.equal(rowView(p({ state: "waiting", fiveHourRemaining: null, weeklyRemaining: null }), NOW, en, "en").wide, false, "waiting keeps its dashes");

    const at = NOW - 47 * MIN;
    const noValidData = { state: "error", staleKind: "stale", severity: "error", updatedAtMs: at } as const;
    const withData = rowView(p(noValidData), NOW, en, "en");
    assert.deepEqual([withData.status, withData.statusTone, withData.wide, withData.five], [`Stale · ${formatTime(at, "en")}`, "critical", false, "88%"], "last values stay visible");
    const zh = rowView(p(noValidData), NOW, zhCN, "zh-CN");
    assert.equal(zh.status, `数据已过时 · ${formatTime(at, "zh-CN")}`);
    assert.equal(zh.note?.text, `已过时 · ${formatTime(at, "zh-CN")}`, "the short form beside two columns");
    assert.equal(zh.label.includes("错误，错误"), false);
    assert.match(zh.label, /错误，数据已过时/);
    const wholeRow = rowView(p({ ...noValidData, fiveHourRemaining: null, weeklyRemaining: null }), NOW, zhCN, "zh-CN");
    assert.deepEqual([wholeRow.wide, wholeRow.note?.text], [true, `数据已过时 · ${formatTime(at, "zh-CN")}`], "the whole row has room for the long form");

    const noTime = rowView(p({ state: "error", severity: "error", updatedAtMs: null }), NOW, zhCN, "zh-CN");
    assert.deepEqual([noTime.status, noTime.statusTone], [zhCN.wError, "critical"], "no time known: only the generic word");
    assert.equal(noTime.label.includes("错误，错误"), false, "said once in the tooltip");
    assert.match(noTime.label, /错误/);
  });
});

describe("widget decisions come from the backend", () => {
  it("severity is said in words in the accessible label, never as a symbol", () => {
    const words = { warning: [/Warning/, /警告/], critical: [/Critical/, /严重/], error: [/Error/, /错误/] } as const;
    for (const [severity, [word, zh]] of Object.entries(words)) {
      assert.match(rowView(p({ severity: severity as WidgetProvider["severity"] }), NOW, en, "en").label, word);
      assert.match(rowView(p({ severity: severity as WidgetProvider["severity"] }), NOW, zhCN, "zh-CN").label, zh);
    }
    assert.equal(rowView(p({ severity: "normal" }), NOW, en, "en").label.includes("("), false, "normal adds nothing");
  });

  it("limit reached only from the backend's availability", () => {
    // 0% left but availability allowed: no "limit reached" (the widget never infers it).
    assert.equal(rowView(p({ fiveHourRemaining: 0 }), NOW, en, "en").status, null);
    // Claude at 0%: availability is always unknown, so never "limit reached".
    assert.equal(rowView(p({ id: "claude", displayName: "Claude", fiveHourRemaining: 0, weeklyRemaining: 0, availability: "unknown" }), NOW, en, "en").status, null);
    const blocked = rowView(p({ fiveHourRemaining: 0, availability: "blocked", severity: "critical", note: "Limit reached" }), NOW, en, "en");
    assert.deepEqual([blocked.status, blocked.statusTone], ["Limit reached", "critical"]);
    assert.equal(rowView(p({ availability: "blocked" }), NOW, zhCN, "zh-CN").status, zhCN.limitReached);
  });

  it("a limit on old data says how old in detailed; compact keeps the limit words alone, the tooltip says it once", () => {
    const at = NOW - 47 * MIN;
    const stale = { state: "error", staleKind: "stale", availability: "blocked", severity: "error", updatedAtMs: at } as const;
    const zhClock = formatTime(at, "zh-CN");
    const old = rowView(p({ ...stale, note: zhCN.limitReached }), NOW, zhCN, "zh-CN");
    assert.deepEqual([old.status, old.note?.text, old.statusTone], [`已达到限制 · 已过时 · ${zhClock}`, zhCN.limitReached, "critical"]);
    assert.match(old.label, new RegExp(`已达到限制，数据已过时 · ${zhClock}`));
    assert.equal(old.label.split(zhClock).length - 1, 1, "the age once");
    assert.equal(old.label.split(zhCN.limitReached).length - 1, 1, "the limit once");
    assert.equal(rowView(p({ ...stale, note: en.limitReached }), NOW, en, "en").status, `Limit reached · Stale · ${formatTime(at, "en")}`);

    const lastWeek = NOW - 3 * DAY;
    const older = rowView(p({ ...stale, note: zhCN.limitReached, updatedAtMs: lastWeek }), NOW, zhCN, "zh-CN");
    assert.deepEqual([older.status, older.note?.text], ["已达到限制 · 数据已过时", zhCN.limitReached], "not today: no time on the row");
    assert.match(older.label, new RegExp(`已达到限制，数据已过时 · ${formatDay(lastWeek, "zh-CN")}`));
    assert.equal(rowView(p({ ...stale, note: en.limitReached, updatedAtMs: lastWeek }), NOW, en, "en").status, "Limit reached · Stale");

    const maybe = { state: "stale", staleKind: "maybe", availability: "blocked", severity: "critical", updatedAtMs: at } as const;
    const e = rowView(p({ ...maybe, note: en.limitReached }), NOW, en, "en");
    assert.deepEqual([e.status, e.note?.text, e.statusTone], ["Limit reached · May be stale", en.limitReached, "critical"]);
    assert.match(e.label, new RegExp(`Limit reached, May be stale · ${formatTime(at, "en")}`));
    assert.equal(rowView(p({ ...maybe, note: zhCN.limitReached }), NOW, zhCN, "zh-CN").status, "已达到限制 · 可能已过时");

    const fresh = rowView(p({ availability: "blocked", severity: "critical", note: en.limitReached }), NOW, en, "en");
    assert.equal(fresh.status, en.limitReached, "fresh data adds nothing");
    assert.equal(fresh.label.includes("Stale"), false);
    assert.equal(fresh.label.includes(formatTime(NOW - 30_000, "en")), false);
  });

  it("compact keeps the limit words alone with the 5-hour column hidden too", () => {
    const hidden = { state: "error", staleKind: "stale", availability: "blocked", severity: "error", note: zhCN.limitReached, updatedAtMs: NOW - 47 * MIN, showFiveHour: false } as const;
    const r = rowView(p(hidden), NOW, zhCN, "zh-CN");
    assert.deepEqual([r.status, r.note?.text], [`已达到限制 · 已过时 · ${formatTime(NOW - 47 * MIN, "zh-CN")}`, zhCN.limitReached]);
    const noValues = rowView(p({ ...hidden, fiveHourRemaining: null, weeklyRemaining: null }), NOW, zhCN, "zh-CN");
    assert.deepEqual([noValues.wide, noValues.note?.text], [true, zhCN.limitReached], "the whole row too: one word for the compact layout");
  });

  it("a limit says nothing of its age while paused: the header says Paused", () => {
    const at = NOW - 3 * HOUR;
    const paused = rowView(p({ state: "paused", staleKind: "stale", availability: "blocked", severity: "critical", note: en.limitReached, updatedAtMs: at }), NOW, en, "en");
    assert.deepEqual([paused.status, paused.note?.text], [en.limitReached, en.limitReached]);
    assert.match(paused.label, new RegExp(`Limit reached, Stale · ${formatTime(at, "en")}`), "the tooltip still knows how old");
  });

  it("only the limit words take an age: a provider error's note stands alone", () => {
    const at = NOW - 47 * MIN;
    const r = rowView(p({ state: "error", staleKind: "stale", availability: "blocked", severity: "error", note: "Cannot refresh Codex usage", noteShort: "Can't refresh", updatedAtMs: at }), NOW, en, "en");
    assert.equal(r.status, "Cannot refresh Codex usage");
    assert.match(r.label, new RegExp(`Error, Cannot refresh Codex usage, Limit reached, Stale · ${formatTime(at, "en")}`), "the tooltip keeps all of it");
  });

  it("a provider error takes the row from a limit; the tooltip still names the limit", () => {
    const failing = { state: "error", availability: "blocked", severity: "error", updatedAtMs: NOW - 2 * MIN } as const;
    const e = rowView(p({ ...failing, note: "Sign in to Codex first", noteShort: "Sign in first" }), NOW, en, "en");
    assert.deepEqual([e.status, e.statusTone], ["Sign in to Codex first", "critical"]);
    assert.match(e.label, /\(Error, Sign in to Codex first, Limit reached\)/);
    const zh = rowView(p({ ...failing, note: "无法刷新 Codex 用量", noteShort: "无法刷新" }), NOW, zhCN, "zh-CN");
    assert.match(zh.label, /（错误，无法刷新 Codex 用量，已达到限制）/);
    const allowed = rowView(p({ ...failing, availability: "allowed", note: "Cannot refresh Codex usage", noteShort: "Can't refresh" }), NOW, en, "en");
    assert.equal(allowed.label.includes("Limit reached"), false, "no flag from the backend, no limit words");
    assert.equal(rowView(p({ availability: "blocked", severity: "critical", note: en.limitReached }), NOW, en, "en").label.split(en.limitReached).length - 1, 1, "the limit's own note is not said twice");
  });

  it("a low value alone never changes the tone (no thresholds in the widget)", () => {
    const r = rowView(p({ fiveHourRemaining: 3, weeklyRemaining: 1 }), NOW, en, "en");
    assert.deepEqual([r.fiveTone, r.weeklyTone, r.status, r.note], ["normal", "normal", null, null]);
  });

  it("a low tone does not raise severity words, and severity does not make a note", () => {
    const low = rowView(p({ fiveHourRemaining: 4, fiveHourTone: "critical" }), NOW, en, "en");
    assert.equal(low.fiveTone, "critical");
    assert.equal(low.label.includes("Critical"), false, "the 5-hour reminder may be off: severity stays the backend's");
    assert.equal(rowView(p({ severity: "warning" }), NOW, en, "en").note, null);
  });
});

describe("a hidden 5-hour limit", () => {
  it("is not spoken, and a shown value without data keeps its dash", () => {
    const hidden = rowView(p({ showFiveHour: false }), NOW, en, "en");
    assert.equal(hidden.showFive, false);
    assert.equal(hidden.label.includes("5-hour"), false);
    assert.equal(rowView(p({ showFiveHour: false }), NOW, zhCN, "zh-CN").label.includes("5 小时"), false);
    const noValue = rowView(p({ fiveHourRemaining: null }), NOW, en, "en");
    assert.equal(noValue.showFive, true);
    assert.equal(noValue.five, "—");
    assert.equal(noValue.label.includes("5-hour"), true);
  });

  it("counts as no value when an error needs the whole row", () => {
    const error = { state: "error" as const, weeklyRemaining: null, note: "Sign in" };
    assert.equal(rowView(p({ ...error, fiveHourRemaining: 40 }), NOW, en, "en").wide, false);
    assert.equal(rowView(p({ ...error, fiveHourRemaining: 40, showFiveHour: false }), NOW, en, "en").wide, true);
  });
});

describe("a backend note beside values", () => {
  // The backend's real strings (product/i18n.rs) in both languages: the sentence, then its short form.
  const notes = [
    ["en", en, "Sign in to Codex first", "Sign in first"],
    ["en", en, "Cannot refresh Codex usage", "Can't refresh"],
    ["en", en, "Codex installation not found", "Not installed"],
    ["zh-CN", zhCN, "请先登录 Codex", "请先登录"],
    ["zh-CN", zhCN, "无法刷新 Codex 用量", "无法刷新"],
    ["zh-CN", zhCN, "未找到 Codex", "未安装"],
  ] as const;
  const failing = { state: "error", severity: "error", updatedAtMs: NOW - 2 * MIN } as const;

  it("takes the short form in the compact slot; status stays the whole sentence for detailed and the tooltip", () => {
    for (const [lang, t, note, noteShort] of notes) {
      const r = rowView(p({ ...failing, note, noteShort }), NOW, t, lang);
      assert.deepEqual([r.note?.text, r.note?.tone, r.status, r.wide], [noteShort, "critical", note, false], note);
      assert.equal(r.label.endsWith(lang === "en" ? `(Error, ${note})` : `（错误，${note}）`), true, "the tooltip has the sentence");
    }
  });

  it("with the 5-hour column hidden too", () => {
    for (const [lang, t, note, noteShort] of notes) {
      const r = rowView(p({ ...failing, note, noteShort, showFiveHour: false }), NOW, t, lang);
      assert.deepEqual([r.note?.text, r.status, r.wide], [noteShort, note, false], note);
    }
  });

  it("on a limit's row as well, whichever the window columns", () => {
    for (const [lang, t, note, noteShort] of notes) {
      for (const showFiveHour of [true, false]) {
        const r = rowView(p({ ...failing, availability: "blocked", fiveHourRemaining: 0, note, noteShort, showFiveHour }), NOW, t, lang);
        assert.deepEqual([r.note?.text, r.status, r.statusTone, r.wide], [noteShort, note, "critical", false], note);
      }
    }
  });

  it("a whole row has room for the sentence", () => {
    for (const [lang, t, note, noteShort] of notes) {
      const r = rowView(p({ ...failing, availability: "unknown", fiveHourRemaining: null, weeklyRemaining: null, note, noteShort }), NOW, t, lang);
      assert.deepEqual([r.wide, r.note?.text, r.status], [true, note, note], note);
      const hidden = rowView(p({ ...failing, availability: "blocked", fiveHourRemaining: 40, weeklyRemaining: null, showFiveHour: false, note, noteShort }), NOW, t, lang);
      assert.deepEqual([hidden.wide, hidden.note?.text], [true, note], "a hidden value counts as none");
    }
  });

  it("the age beside a note's row keeps its own rule: short beside two columns, whole with the column hidden", () => {
    const at = NOW - 47 * MIN;
    const stale = { state: "error", staleKind: "stale", severity: "error", updatedAtMs: at, noteShort: null } as const;
    assert.equal(rowView(p(stale), NOW, zhCN, "zh-CN").note?.text, `已过时 · ${formatTime(at, "zh-CN")}`);
    assert.equal(rowView(p({ ...stale, showFiveHour: false }), NOW, zhCN, "zh-CN").note?.text, `数据已过时 · ${formatTime(at, "zh-CN")}`);
  });
});

describe("the compact note", () => {
  const low = { fiveHourRemaining: 12, fiveHourTone: "low", weeklyRemaining: 25, weeklyTone: "low" } as const;
  const claude = { id: "claude", displayName: "Claude", availability: "unknown" } as const;
  const exact = (inMs: number): WidgetProvider["weeklyReset"] => ({ schedule: "confirmed", atMs: NOW + inMs, precisionMs: 1000 });
  const minute = (inMs: number): WidgetProvider["weeklyReset"] => ({ schedule: "confirmed", atMs: NOW + inMs, precisionMs: MIN });

  it("only for a shown low or critical window, with the least remaining, ties to 5-hour", () => {
    assert.equal(rowView(p({}), NOW, en, "en").note, null);
    assert.equal(rowView(p({ ...low, fiveHourReset: exact(2 * HOUR) }), NOW, en, "en").note?.text.startsWith("5h "), true);
    assert.equal(rowView(p({ ...low, fiveHourRemaining: 28 }), NOW, en, "en").note?.text.startsWith("Week "), true);
    assert.equal(rowView(p({ ...low, fiveHourRemaining: 25 }), NOW, zhCN, "zh-CN").note?.text.startsWith("5小时 "), true);
    assert.equal(rowView(p({ fiveHourRemaining: 12, fiveHourTone: "low" }), NOW, en, "en").note?.text.startsWith("5h "), true);
    assert.equal(rowView(p({ ...low, showFiveHour: false }), NOW, en, "en").note?.text.startsWith("Week "), true, "a hidden window never names itself");
  });

  it("takes the window's tone in words the colour goes with", () => {
    assert.equal(rowView(p({ ...low }), NOW, en, "en").note?.tone, "warning");
    assert.equal(rowView(p({ ...low, fiveHourTone: "critical" }), NOW, en, "en").note?.tone, "critical");
  });

  it("a status note takes priority over the low note", () => {
    assert.equal(rowView(p({ ...low, availability: "blocked" }), NOW, zhCN, "zh-CN").note?.text, zhCN.limitReached);
    assert.equal(rowView(p({ ...low, state: "error", note: "Sign in" }), NOW, en, "en").note?.text, "Sign in");
    const stale = { ...low, state: "stale", staleKind: "stale" } as const;
    assert.equal(rowView(p({ ...stale, updatedAtMs: NOW - 47 * MIN }), NOW, zhCN, "zh-CN").note?.text, `已过时 · ${formatTime(NOW - 47 * MIN, "zh-CN")}`, "the short stale form beside two columns");
    assert.equal(rowView(p({ ...stale, updatedAtMs: NOW - 47 * MIN, showFiveHour: false }), NOW, zhCN, "zh-CN").note?.text, `数据已过时 · ${formatTime(NOW - 47 * MIN, "zh-CN")}`);
    assert.equal(rowView(p({ ...stale, updatedAtMs: NOW - 3 * DAY }), NOW, zhCN, "zh-CN").note?.text, "数据已过时", "an older date does not fit beside two columns");
  });

  it("beside two columns: the clock under 24 h, the day or date further out", () => {
    const at = (reset: WidgetProvider["weeklyReset"], lang: "en" | "zh-CN" = "zh-CN") => rowView(p({ ...claude, ...low, fiveHourReset: reset, weeklyReset: reset }), NOW, lang === "en" ? en : zhCN, lang).note?.text;
    assert.equal(at(exact(2 * HOUR + 13 * MIN)), `5小时 ${formatTime(NOW + 2 * HOUR + 13 * MIN, "zh-CN")}`);
    assert.equal(at(minute(2 * HOUR)), `5小时 约${formatTime(NOW + 2 * HOUR, "zh-CN")}`);
    assert.equal(at(minute(2 * HOUR), "en"), `5h ~${formatTime(NOW + 2 * HOUR, "en")}`);
    const weekly = (reset: WidgetProvider["weeklyReset"], lang: "en" | "zh-CN" = "zh-CN") => rowView(p({ ...claude, ...low, fiveHourTone: "normal", fiveHourRemaining: 80, weeklyReset: reset }), NOW, lang === "en" ? en : zhCN, lang).note?.text;
    const weekday = new Date(NOW + 3 * DAY).getDay();
    assert.equal(weekly(exact(3 * DAY)), `每周 ${zhCN.weekdays[weekday]}`);
    assert.equal(weekly(exact(3 * DAY), "en"), `Week ${en.weekdays[weekday]}`);
    assert.equal(weekly(minute(3 * DAY)), `每周 ${zhCN.weekdays[weekday]}`, "no minutes left, so no approximate mark");
    assert.equal(weekly(exact(8 * DAY)), `每周 ${formatDay(NOW + 8 * DAY, "zh-CN")}`);
  });

  it("under 24 h but not today: the bare weekday, never a clock that reads as already gone", () => {
    // 2026-11-03 is a Tuesday.
    const tue = (h: number, m = 0) => new Date(2026, 10, 3, h, m).getTime();
    const conf = (atMs: number, precisionMs: number | null = 1000): WidgetProvider["weeklyReset"] => ({ schedule: "confirmed", atMs, precisionMs });
    const weekly = (now: number, reset: WidgetProvider["weeklyReset"], lang: "en" | "zh-CN" = "zh-CN", over: Partial<WidgetProvider> = {}) => rowView(p({ ...claude, ...low, fiveHourTone: "normal", fiveHourRemaining: 80, weeklyReset: reset, ...over }), now, lang === "en" ? en : zhCN, lang);
    const wed1430 = new Date(2026, 10, 4, 14, 30).getTime();
    assert.equal(weekly(tue(20), conf(wed1430)).note?.text, "每周 周三");
    assert.equal(weekly(tue(20), conf(wed1430), "en").note?.text, "Week Wed");
    assert.equal(weekly(tue(20), conf(wed1430, MIN)).note?.text, "每周 周三", "no time, so no approximate mark");
    assert.equal(weekly(tue(13), conf(tue(20))).note?.text, `每周 ${formatTime(tue(20), "zh-CN")}`, "today keeps its clock");
    const five = rowView(p({ ...claude, ...low, fiveHourReset: conf(new Date(2026, 10, 4, 1, 0).getTime()) }), tue(22), en, "en");
    assert.equal(five.note?.text, "5h Wed", "a 5-hour window crossing midnight");
    const row = weekly(tue(20), conf(wed1430));
    assert.equal(row.weeklyReset, "18小时30分后", "the detailed layout keeps its phrase");
    assert.match(row.label, /18 小时 30 分后重置/, "and so does the tooltip");
    assert.equal(weekly(tue(20), conf(wed1430), "zh-CN", { showFiveHour: false }).note?.text, "每周 18小时30分后", "room for the full phrase");
  });

  it("on the weekly reset day the next reset takes its date, not today's weekday", () => {
    const tue16 = new Date(2026, 10, 3, 16, 0).getTime();
    const next = new Date(2026, 10, 10, 15, 0).getTime();
    const note = (at: number, lang: "en" | "zh-CN", now = tue16) => rowView(p({ ...claude, ...low, fiveHourTone: "normal", fiveHourRemaining: 80, weeklyReset: { schedule: "confirmed", atMs: at, precisionMs: 1000 } }), now, lang === "en" ? en : zhCN, lang).note?.text;
    assert.equal(note(next, "zh-CN"), "每周 11月10日");
    assert.equal(note(next, "en"), "Week Nov 10");
    assert.equal(note(new Date(2026, 10, 9, 15, 0).getTime(), "zh-CN"), "每周 周一", "another weekday stays a weekday");
  });

  it("with the 5-hour column hidden there is room for the full phrase", () => {
    const r = rowView(p({ ...claude, ...low, showFiveHour: false, weeklyReset: exact(2 * HOUR + 13 * MIN) }), NOW, zhCN, "zh-CN");
    assert.equal(r.note?.text, "每周 2小时13分后");
  });

  it("no confirmed time: never a clock", () => {
    const none = (schedule: "sliding" | "unconfirmed" | "passed"): WidgetProvider["weeklyReset"] => ({ schedule, atMs: null, precisionMs: null });
    const note = (reset: WidgetProvider["weeklyReset"], t: typeof en, lang: "en" | "zh-CN") => rowView(p({ ...low, fiveHourReset: reset }), NOW, t, lang).note?.text;
    assert.deepEqual(["sliding", "unconfirmed", "passed"].map((s) => note(none(s as "sliding"), zhCN, "zh-CN")), ["5小时 空闲", "5小时 待确认", "5小时 等待更新"]);
    assert.deepEqual(["sliding", "unconfirmed", "passed"].map((s) => note(none(s as "sliding"), en, "en")), ["5h Idle", "5h TBC", "5h Waiting"]);
    assert.equal(note(exact(-MIN), en, "en"), "5h Waiting", "passed between polls");
  });
});

describe("resets in the row", () => {
  it("detailed phrases come from the one formatter, every state", () => {
    const r = rowView(p({ fiveHourReset: { schedule: "confirmed", atMs: NOW + 2 * HOUR + 13 * MIN, precisionMs: null }, weeklyReset: { schedule: "unconfirmed", atMs: null, precisionMs: null } }), NOW, zhCN, "zh-CN");
    assert.deepEqual([r.fiveReset, r.weeklyReset], ["2小时13分后", "待确认"]);
    const e = rowView(p({ fiveHourReset: { schedule: "sliding", atMs: null, precisionMs: null }, weeklyReset: { schedule: "missing", atMs: null, precisionMs: null } }), NOW, en, "en");
    assert.deepEqual([e.fiveReset, e.weeklyReset], ["Starts on first use", "Not reported"]);
  });

  it("the tooltip names both resets and the severity; a hidden window is left out", () => {
    const provider = p({ severity: "warning", fiveHourReset: { schedule: "confirmed", atMs: NOW + 2 * HOUR + 13 * MIN, precisionMs: null }, weeklyReset: { schedule: "passed", atMs: null, precisionMs: null } });
    const zh = rowView(provider, NOW, zhCN, "zh-CN").label;
    assert.match(zh, /5 小时 剩余 88%，2 小时 13 分后重置/);
    assert.match(zh, /每周 剩余 82%，已到重置时间，等待下一次更新/);
    assert.match(zh, /警告/);
    const hidden = rowView({ ...provider, showFiveHour: false }, NOW, en, "en").label;
    assert.equal(hidden.includes("2 h 13 min"), false);
    assert.match(hidden, /Reset time passed/);
  });
});

describe("widget header and formatting", () => {
  it("the compact header only ever says Paused; with no provider there is no header", () => {
    assert.equal(headerStatus(view({}), NOW, en), null, "no age: the columns are there");
    assert.equal(headerStatus(view({ updatedAtMs: null }), NOW, en), null);
    assert.equal(headerStatus(view({ paused: true, providers: [] }), NOW, en), null, "no header: only the hint");
  });

  it("formats ages and values", () => {
    assert.equal(ageText(NOW - 20_000, NOW, en), "just now");
    assert.equal(ageText(NOW + 5_000, NOW, en), "just now", "a clock skew never shows a negative age");
    assert.equal(ageText(NOW - 119 * MIN, NOW, en), "119m ago");
    assert.equal(ageText(null, NOW, en), null);
    assert.equal(remainingText(null), "—");
    assert.equal(barWidth(140), 100);
    assert.equal(barWidth(-3), 0);
  });
});

describe("widget ring layout", () => {
  it("weekly is the big number and the outer ring; 5-hour has the inner ring and its own line", () => {
    const r = ringView(p({ fiveHourRemaining: 74, weeklyRemaining: 73 }), NOW, zhCN, "zh-CN");
    assert.deepEqual([r.weekly, r.weeklyValue, r.five?.text, r.five?.value], ["73%", 73, "74%", 74]);
    assert.match(r.line.text, /^周. \d{2}:\d{2} 重置$/, "the line under the number is the weekly reset, with its verb");
    assert.match(ringView(p({}), NOW, en, "en").line.text, /^Resets \w{3} \d{2}:\d{2}$/);
    const soon = ringView(p({ weeklyReset: { schedule: "confirmed", atMs: NOW + 5 * 60 * MIN, precisionMs: null } }), NOW, zhCN, "zh-CN");
    assert.equal(soon.line.text, "5小时后重置");
    const approx = ringView(p({ weeklyReset: { schedule: "confirmed", atMs: NOW + 3 * 24 * 60 * MIN, precisionMs: 3_600_000 } }), NOW, zhCN, "zh-CN");
    assert.match(approx.line.text, /^预计 .+ 重置$/, "a coarse reset stays expected");
    assert.equal(r.five?.reset, rowView(p({}), NOW, zhCN, "zh-CN").fiveReset, "the widget's own 5-hour phrase");
  });

  it("a hidden 5-hour window leaves no inner ring and no 5-hour line", () => {
    assert.equal(ringView(p({ showFiveHour: false }), NOW, en, "en").five, null);
  });

  it("an expected 5-hour reset is a bare clock after a tilde, an exact one a countdown", () => {
    const at = NOW + 3 * 60 * MIN;
    const expected = ringView(p({ fiveHourReset: { schedule: "confirmed", atMs: at, precisionMs: 60_000 } }), NOW, zhCN, "zh-CN");
    assert.equal(expected.five?.reset, `~${formatTime(at, "zh-CN")}`);
    const exact = ringView(p({ fiveHourReset: { schedule: "confirmed", atMs: at, precisionMs: 1000 } }), NOW, zhCN, "zh-CN");
    assert.equal(exact.five?.reset, "3小时后");
  });

  it("a 5-hour line without a value names no reset", () => {
    const r = ringView(p({ fiveHourRemaining: null, fiveHourReset: { schedule: "missing", atMs: null, precisionMs: null } }), NOW, en, "en");
    assert.deepEqual([r.five?.text, r.five?.reset], ["—", null]);
  });

  it("a status takes the line under the number, in its tone; the weekly reset moves to the tooltip", () => {
    const stale = ringView(p({ state: "stale", staleKind: "stale", updatedAtMs: NOW - 30 * 60 * MIN }), NOW, en, "en");
    assert.equal(stale.line.tone, "critical");
    assert.equal(stale.line.text, rowView(p({ state: "stale", staleKind: "stale", updatedAtMs: NOW - 30 * 60 * MIN }), NOW, en, "en").status);
    const blocked = ringView(p({ availability: "blocked" }), NOW, zhCN, "zh-CN");
    assert.deepEqual(blocked.line, { text: "已达到限制", tone: "critical" });
    assert.match(blocked.label, /3天|周/, "the tooltip still has the weekly reset");
    const waiting = ringView(p({ state: "waiting", fiveHourRemaining: null, weeklyRemaining: null }), NOW, en, "en");
    assert.deepEqual([waiting.weekly, waiting.line.text, waiting.line.tone], ["—", "Waiting", "muted"]);
  });

  it("a limit says the short words and how old its data is; a provider error and a backend note take their short form", () => {
    const old = ringView(p({ availability: "blocked", state: "stale", staleKind: "maybe", updatedAtMs: NOW - 40 * MIN }), NOW, en, "en");
    assert.deepEqual(old.line, { text: "At limit · May be stale", tone: "critical" });
    const err = ringView(p({ availability: "blocked", note: "Cannot refresh Codex usage", noteShort: "Can't refresh" }), NOW, en, "en");
    assert.equal(err.line.text, "Can't refresh");
    const missing = ringView(p({ state: "error", note: "Codex installation not found", noteShort: "Not installed", fiveHourRemaining: null, weeklyRemaining: null }), NOW, en, "en");
    assert.deepEqual(missing.line, { text: "Not installed", tone: "critical" });
  });

  it("the 5-hour line names only a time left; other reset words stay in the tooltip", () => {
    for (const schedule of ["unconfirmed", "sliding", "passed"] as const) {
      const r = ringView(p({ fiveHourReset: { schedule, atMs: null, precisionMs: null } }), NOW, en, "en");
      assert.equal(r.five?.reset, null, schedule);
    }
  });

  it("paused mutes both rings and says so on the row", () => {
    const r = ringView(p({ state: "paused", weeklyTone: "low" }), NOW, en, "en");
    assert.deepEqual([r.weeklyTone, r.five?.tone, r.line.text, r.line.tone], ["muted", "muted", "Paused", "muted"]);
  });
});
