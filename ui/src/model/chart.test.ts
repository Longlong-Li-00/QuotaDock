import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { HistoryPoint } from "../api/types.ts";
import { buildPaths, chartDomain, commonGaps, firstTime, gapRanges, mergeRanges, pointAt, yOf, type Frame } from "./chart.ts";

const f: Frame = { t0: 0, t1: 100, width: 120, height: 120, padLeft: 10, padRight: 10, padTop: 10, padBottom: 10 };
const p = (t: number, remaining: number, segment = 1, gapBefore = false): HistoryPoint => ({ t, remaining, used: 100 - remaining, segment, gapBefore });

describe("history chart geometry", () => {
  it("draws one continuous path within a segment", () => {
    const paths = buildPaths([p(0, 90), p(50, 80), p(100, 70)], (x) => x.remaining, f);
    assert.equal(paths.length, 1);
    assert.equal((paths[0]!.match(/L/g) ?? []).length, 2);
  });

  it("breaks the line at an observed-cycle boundary instead of drawing 2% -> 100%", () => {
    const paths = buildPaths([p(0, 4), p(40, 2), p(60, 100, 2), p(100, 97, 2)], (x) => x.remaining, f);
    assert.equal(paths.length, 2);
    assert.ok(paths[1]!.startsWith("M"), "the new segment starts with a move, not a line");
  });

  it("breaks the line across a measurement gap", () => {
    assert.equal(buildPaths([p(0, 90), p(10, 89), p(90, 70, 1, true)], (x) => x.remaining, f).length, 2);
  });

  it("maps 0-100% to the plot area and clamps", () => {
    assert.equal(yOf(100, f), 10);
    assert.equal(yOf(0, f), 110);
    assert.equal(yOf(140, f), 10);
  });

  it("handles empty input", () => {
    assert.deepEqual(buildPaths([], (x) => x.remaining, f), []);
  });
});

describe("history chart domain, gaps and hover values", () => {
  const H = 3_600_000;
  const day = { from: 0, to: 24 * H };

  it("keeps the full range when data covers it", () => {
    assert.deepEqual(chartDomain(day.from, day.to, 1 * H), { t0: 0, t1: 24 * H, trimmed: false });
  });

  it("trims a mostly empty range to start just before the first data", () => {
    const d = chartDomain(day.from, day.to, 19 * H); // data only in the last 5 hours
    assert.equal(d.trimmed, true);
    assert.ok(d.t0 < 19 * H && d.t0 > 18.5 * H, `starts shortly before 19:00, got ${d.t0 / H} h`);
    assert.equal(d.t1, 24 * H);
    assert.deepEqual(chartDomain(day.from, day.to, null), { t0: 0, t1: 24 * H, trimmed: false });
  });

  it("finds the earliest point across series", () => {
    assert.equal(firstTime([{ points: [p(5, 1), p(9, 1)] }, { points: [p(3, 1)] }, { points: [] }]), 3);
    assert.equal(firstTime([]), null);
  });

  it("lists and merges measurement gaps", () => {
    const a = gapRanges([p(0, 90), p(10, 89), p(50, 80, 1, true), p(60, 79), p(90, 70, 1, true)]);
    assert.deepEqual(a, [[10, 50], [60, 90]]);
    assert.deepEqual(mergeRanges([[60, 90], [10, 50], [40, 55]]), [[10, 55], [60, 90]]);
  });

  it("shades only where no series has data", () => {
    const codex = [p(0, 90), p(30, 89), p(55, 80, 1, true), p(60, 79)]; // gap 30–55
    const claude = [p(0, 90), p(20, 89), p(60, 80, 1, true)]; // gap 20–60
    assert.deepEqual(commonGaps([codex, claude]), [[30, 55]]);
    assert.deepEqual(commonGaps([claude]), [[20, 60]]);
    assert.deepEqual(commonGaps([codex, [p(0, 1), p(60, 1)]]), [], "one series has data throughout");
    assert.deepEqual(commonGaps([]), []);
  });

  it("reads the nearest value within the tolerance only", () => {
    const pts = [p(0, 90), p(10, 80), p(20, 70)];
    assert.equal(pointAt(pts, 12, 5)?.remaining, 80);
    assert.equal(pointAt(pts, 16, 5)?.remaining, 70);
    assert.equal(pointAt(pts, 40, 5), null, "beyond the last point");
    assert.equal(pointAt([p(0, 90), p(100, 50, 1, true)], 50, 20), null, "inside a gap");
    assert.equal(pointAt([], 5, 5), null);
  });
});
