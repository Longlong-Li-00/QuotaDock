import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { buildCodexSnapshot } from "../src/codex/parser/buildSnapshot.ts";
import { detectQuotaTransition } from "../src/codex/transition.ts";
import { loadCase } from "./helpers/fixtures.ts";

const ctx = { capturedAt: "2026-10-07T00:00:00.000Z", codexCliVersion: "0.162.0-alpha.2", transport: "stdio-app-server" as const };
const snap = (raw: unknown) => buildCodexSnapshot(raw, ctx);

describe("detectQuotaTransition", () => {
  it("reports nothing without a previous snapshot or when nothing changed", () => {
    const a = snap(loadCase("caseA_standard"));
    for (const t of [detectQuotaTransition(null, a), detectQuotaTransition(a, snap(loadCase("caseA_standard")))]) {
      assert.equal(t.percentageChanged, false);
      assert.equal(t.resetTimestampChanged, false);
      assert.equal(t.bucketTopologyChanged, false);
      assert.equal(t.resetCandidate, false);
      assert.deepEqual(t.changes, []);
    }
  });

  it("flags a drop in usedPercent as a reset candidate only (no classification)", () => {
    const before = loadCase("caseA_standard") as any;
    const after = structuredClone(before);
    after.rateLimitsByLimitId.codex.primary.usedPercent = 0;
    after.rateLimitsByLimitId.codex.primary.resetsAt = 1791418000;
    const t = detectQuotaTransition(snap(before), snap(after));
    assert.equal(t.percentageChanged, true);
    assert.equal(t.resetCandidate, true);
    assert.equal(t.resetTimestampChanged, true);
    assert.equal(t.bucketTopologyChanged, false);
    assert.deepEqual(t.changes.map((c) => `${c.bucket}.${c.slot}.${c.field}`), ["codex.primary.usedPercent", "codex.primary.resetsAt"]);
    assert.equal("resetType" in t, false);
  });

  it("an increase is a percentage change but not a reset candidate", () => {
    const before = loadCase("caseA_standard") as any;
    const after = structuredClone(before);
    after.rateLimitsByLimitId.codex.secondary.usedPercent = 62;
    const t = detectQuotaTransition(snap(before), snap(after));
    assert.equal(t.percentageChanged, true);
    assert.equal(t.resetCandidate, false);
  });

  it("detects bucket topology changes (new bucket, window disappearing, duration change)", () => {
    const a = snap(loadCase("caseA_standard"));
    const d = snap(loadCase("caseD_additionalBucket"));
    assert.equal(detectQuotaTransition(a, d).bucketTopologyChanged, true);

    const c = snap(loadCase("caseC_fiveHourOnly"));
    const t = detectQuotaTransition(a, c);
    assert.equal(t.bucketTopologyChanged, true);
    assert.ok(t.changes.some((x) => x.slot === "secondary" && x.field === "presence"));

    const g = snap(loadCase("caseG_unknownDuration"));
    assert.ok(detectQuotaTransition(a, g).changes.some((x) => x.field === "windowDurationMins"));
  });

  it("notes availability changes separately", () => {
    const allowed = snap(loadCase("caseA_standard"));
    const blocked = snap(loadCase("caseE_blockedResetInPast"));
    assert.equal(detectQuotaTransition(allowed, blocked).availabilityChanged, true);
  });
});
