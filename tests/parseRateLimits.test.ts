import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { buildCodexSnapshot, selectCodexBucket, toAvailability } from "../src/codex/parser/buildSnapshot.ts";
import { computeRemainingPercent, parseRateLimitsResponse, RateLimitParseError, resetsAtToIso } from "../src/codex/parser/parseRateLimits.ts";
import { loadCase, loadReal } from "./helpers/fixtures.ts";

const ctx = { capturedAt: "2026-10-07T14:32:31.163Z", codexCliVersion: "0.162.0-alpha.2", transport: "stdio-app-server" as const };

describe("real redacted fixture (codex-cli 0.162.0-alpha.2)", () => {
  const real = loadReal();

  it("parses every bucket and window", () => {
    const parsed = parseRateLimitsResponse(real.response);
    assert.equal(parsed.ordinaryUsageAllowed, true);
    assert.deepEqual(Object.keys(parsed.byLimitId ?? {}), ["codex"]);
    assert.equal(parsed.legacy?.limitId, "codex");
    assert.deepEqual(parsed.warnings, []);
    assert.equal(parsed.resetCredits?.availableCount, 2);
  });

  it("builds a snapshot from the codex bucket", () => {
    const s = buildCodexSnapshot(real.response, ctx);
    assert.equal(s.provider, "codex");
    assert.equal(s.sourceBucket, "codex");
    assert.equal(s.availability, "allowed");
    assert.equal(s.fiveHour?.windowDurationMins, 300);
    assert.equal(s.weekly?.windowDurationMins, 10080);
    assert.equal(s.fiveHour?.remainingPercent, 100 - (s.fiveHour?.usedPercent ?? NaN));
    assert.equal(typeof s.fiveHour?.resetsAt, "number");
    assert.equal(s.fiveHour?.resetsAtIso, new Date(s.fiveHour!.resetsAt! * 1000).toISOString());
    assert.equal(s.accountId, "[REDACTED]");
    assert.equal(s.raw, undefined, "raw only when requested");
    assert.deepEqual(s.warnings, []);
  });
});

describe("parseRateLimitsResponse / buildCodexSnapshot", () => {
  it("Case D: additional buckets are all kept; normalized view comes from codex only", () => {
    const s = buildCodexSnapshot(loadCase("caseD_additionalBucket"), ctx);
    assert.deepEqual(Object.keys(s.buckets).sort(), ["codex", "zz_metered_feature"]);
    assert.equal(s.fiveHour?.usedPercent, 10, "not the 70% of the other bucket");
    const other = s.buckets.zz_metered_feature!;
    assert.equal(other.primary?.usedPercent, 70);
    assert.equal((other.raw as Record<string, unknown>).limitName, "Some Feature", "raw bucket kept verbatim");
  });

  it("Case E: ordinaryUsageAllowed=false with resetsAt in the past stays blocked", () => {
    const s = buildCodexSnapshot(loadCase("caseE_blockedResetInPast"), ctx);
    assert.ok(s.fiveHour!.resetsAt! * 1000 < Date.now(), "fixture reset time is in the past");
    assert.equal(s.ordinaryUsageAllowed, false);
    assert.equal(s.availability, "blocked");
    assert.equal(s.fiveHour?.resetsAt, 1700000000, "timestamp kept verbatim");
    assert.equal(s.fiveHour?.usedPercent, 100);
  });

  it("Case F: ordinaryUsageAllowed=null -> unknown", () => {
    const s = buildCodexSnapshot(loadCase("caseF_availabilityUnknown"), ctx);
    assert.equal(s.ordinaryUsageAllowed, null);
    assert.equal(s.availability, "unknown");
    assert.equal(s.accountId, null);
  });

  it("toAvailability maps only from the boolean", () => {
    assert.equal(toAvailability(true), "allowed");
    assert.equal(toAvailability(false), "blocked");
    assert.equal(toAvailability(null), "unknown");
    assert.equal(toAvailability(undefined), "unknown");
  });

  it("legacy fallback when rateLimitsByLimitId is absent", () => {
    const s = buildCodexSnapshot(loadCase("legacyOnly"), ctx);
    assert.equal(s.sourceBucket, "legacy");
    assert.deepEqual(s.buckets, {});
    assert.equal(s.fiveHour?.usedPercent, 15);
    assert.equal(s.weekly?.usedPercent, 25);
    assert.match(s.warnings.join("\n"), /legacy/);
  });

  it("never treats the first bucket as codex", () => {
    const parsed = parseRateLimitsResponse(loadCase("noCodexBucket"));
    const sel = selectCodexBucket(parsed);
    assert.equal(sel.bucket, null);
    assert.equal(sel.source, null);
    const s = buildCodexSnapshot(loadCase("noCodexBucket"), ctx);
    assert.equal(s.fiveHour, null);
    assert.equal(s.weekly, null);
    assert.ok(s.buckets.other_feature, "unknown bucket still kept");
  });

  it("keeps out-of-range values as reported and refuses implausible timestamps", () => {
    const s = buildCodexSnapshot(loadCase("outOfRangeValues"), ctx);
    assert.equal(s.ordinaryUsageAllowed, null, "non-boolean is unknown, not truthy");
    assert.equal(s.fiveHour?.usedPercent, 104.5, "usedPercent never rewritten");
    assert.equal(s.fiveHour?.remainingPercent, 0, "remaining clamped");
    assert.equal(s.fiveHour?.resetsAt, 1791400000000, "raw timestamp kept");
    assert.equal(s.fiveHour?.resetsAtIso, null, "milliseconds-looking value not converted");
    assert.equal(s.weekly?.usedPercent, null, "string percent is not coerced");
    assert.equal(s.weekly?.resetsAt, null);
    const w = s.warnings.join("\n");
    assert.match(w, /ordinaryUsageAllowed is not a boolean/);
    assert.match(w, /outside 0\.\.100/);
    assert.match(w, /plausible Unix-seconds range/);
    assert.match(w, /usedPercent is not a number/);
  });

  it("includes raw only when asked", () => {
    const raw = loadCase("caseA_standard");
    assert.deepEqual(buildCodexSnapshot(raw, { ...ctx, includeRaw: true }).raw, raw);
  });

  it("rejects a non-object result", () => {
    assert.throws(() => parseRateLimitsResponse(null), RateLimitParseError);
    assert.throws(() => parseRateLimitsResponse([1, 2]), RateLimitParseError);
  });

  it("remainingPercent = clamp(100 - used)", () => {
    assert.equal(computeRemainingPercent(28), 72);
    assert.equal(computeRemainingPercent(0), 100);
    assert.equal(computeRemainingPercent(120), 0);
    assert.equal(computeRemainingPercent(-5), 100);
    assert.equal(computeRemainingPercent(null), null);
  });

  it("resetsAtToIso treats values as Unix seconds", () => {
    assert.equal(resetsAtToIso(1791393584), "2026-10-07T17:19:44.000Z");
    assert.equal(resetsAtToIso(null), null);
  });
});
