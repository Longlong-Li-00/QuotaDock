import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { classifyWindow, normalizeCodexWindows, type KnownWindowClass } from "../src/codex/parser/normalizeWindows.ts";
import { parseRateLimitsResponse } from "../src/codex/parser/parseRateLimits.ts";
import type { QuotaWindow, RateLimitBucket } from "../src/codex/types.ts";
import { loadCase } from "./helpers/fixtures.ts";

function codexBucket(caseName: string): RateLimitBucket {
  const parsed = parseRateLimitsResponse(loadCase(caseName));
  const bucket = parsed.byLimitId?.codex;
  assert.ok(bucket, `fixture ${caseName} has a codex bucket`);
  return bucket;
}

const win = (windowDurationMins: number | null): QuotaWindow => ({
  usedPercent: 1,
  remainingPercent: 99,
  windowDurationMins,
  resetsAt: 1791400000,
  resetsAtIso: "2026-10-07T19:06:40.000Z",
});

describe("classifyWindow", () => {
  it("classifies by duration only", () => {
    assert.equal(classifyWindow(win(300)), "five-hour");
    assert.equal(classifyWindow(win(10080)), "weekly");
    assert.equal(classifyWindow(win(1440)), "unknown");
    assert.equal(classifyWindow(win(null)), "unknown");
  });

  it("accepts an extended duration table for future windows", () => {
    const table = new Map<number, KnownWindowClass>([[600, "five-hour"]]);
    assert.equal(classifyWindow(win(600), table), "five-hour");
    assert.equal(classifyWindow(win(300), table), "unknown");
  });
});

describe("normalizeCodexWindows", () => {
  it("Case A: primary=300, secondary=10080", () => {
    const n = normalizeCodexWindows(codexBucket("caseA_standard"));
    assert.equal(n.fiveHour?.usedPercent, 28);
    assert.equal(n.fiveHour?.windowDurationMins, 300);
    assert.equal(n.weekly?.usedPercent, 61);
    assert.equal(n.weekly?.windowDurationMins, 10080);
    assert.deepEqual(n.unknown, []);
    assert.deepEqual(n.warnings, []);
  });

  it("Case B: primary=10080, secondary=null -> weekly found, fiveHour null", () => {
    const n = normalizeCodexWindows(codexBucket("caseB_weeklyInPrimaryOnly"));
    assert.equal(n.weekly?.usedPercent, 40);
    assert.equal(n.weekly?.windowDurationMins, 10080);
    assert.equal(n.fiveHour, null);
  });

  it("Case C: primary=300, secondary=null -> weekly stays null (never invented)", () => {
    const n = normalizeCodexWindows(codexBucket("caseC_fiveHourOnly"));
    assert.equal(n.fiveHour?.usedPercent, 5);
    assert.equal(n.weekly, null);
  });

  it("Case G: unknown duration 1440 is kept as unknown, not force-classified", () => {
    const n = normalizeCodexWindows(codexBucket("caseG_unknownDuration"));
    assert.equal(n.fiveHour, null);
    assert.equal(n.weekly?.windowDurationMins, 10080);
    assert.equal(n.unknown.length, 1);
    assert.equal(n.unknown[0]?.slot, "primary");
    assert.equal(n.unknown[0]?.window.windowDurationMins, 1440);
    assert.match(n.warnings.join("\n"), /1440/);
  });

  it("does not depend on slot position (weekly in primary, 5h in secondary)", () => {
    const n = normalizeCodexWindows(codexBucket("swappedSlots"));
    assert.equal(n.fiveHour?.usedPercent, 28);
    assert.equal(n.weekly?.usedPercent, 61);
  });

  it("two windows with duration 300 -> no arbitrary pick, warning recorded", () => {
    const n = normalizeCodexWindows(codexBucket("duplicateFiveHour"));
    assert.equal(n.fiveHour, null);
    assert.match(n.warnings.join("\n"), /2 windows classify as five-hour/);
  });

  it("null bucket yields an empty view", () => {
    const n = normalizeCodexWindows(null);
    assert.equal(n.fiveHour, null);
    assert.equal(n.weekly, null);
  });
});
