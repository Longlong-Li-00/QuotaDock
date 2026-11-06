// Pure chart geometry for the history chart (SVG). Lines are broken where the core marks a new
// observed-cycle segment or a measurement gap, so a reset never looks like a continuous climb.

import type { HistoryPoint } from "../api/types.ts";

export interface Frame {
  t0: number;
  t1: number;
  width: number;
  height: number;
  padLeft: number;
  padRight: number;
  padTop: number;
  padBottom: number;
}

export function xOf(t: number, f: Frame): number {
  const span = Math.max(1, f.t1 - f.t0);
  return f.padLeft + ((t - f.t0) / span) * (f.width - f.padLeft - f.padRight);
}

export function yOf(v: number, f: Frame): number {
  const clamped = Math.max(0, Math.min(100, v));
  return f.padTop + (1 - clamped / 100) * (f.height - f.padTop - f.padBottom);
}

/** One SVG path per contiguous run; a run ends at a segment change or a gap. */
export function buildPaths(points: HistoryPoint[], value: (p: HistoryPoint) => number, f: Frame): string[] {
  const paths: string[] = [];
  let current = "";
  let prev: HistoryPoint | null = null;
  for (const p of points) {
    const xy = `${xOf(p.t, f).toFixed(1)} ${yOf(value(p), f).toFixed(1)}`;
    const breaks = prev === null || p.segment !== prev.segment || p.gapBefore;
    if (breaks) {
      if (current) paths.push(current);
      current = `M${xy}`;
    } else {
      current += ` L${xy}`;
    }
    prev = p;
  }
  if (current) paths.push(current);
  return paths;
}

/** Time of the earliest point of any series, or null when there is none. */
export function firstTime(series: { points: HistoryPoint[] }[]): number | null {
  let first: number | null = null;
  for (const s of series) {
    const t = s.points[0]?.t;
    if (t !== undefined && (first === null || t < first)) first = t;
  }
  return first;
}

/**
 * The visible time span: the requested range, trimmed to start just before the first data when
 * more than a tenth of it would otherwise be empty (e.g. nothing was recorded yet).
 */
export function chartDomain(fromMs: number, toMs: number, first: number | null): { t0: number; t1: number; trimmed: boolean } {
  if (first === null || first - fromMs <= 0.1 * (toMs - fromMs)) return { t0: fromMs, t1: toMs, trimmed: false };
  const pad = Math.max(60_000, 0.03 * (toMs - first));
  return { t0: Math.max(fromMs, first - pad), t1: toMs, trimmed: true };
}

/** Intervals without measurements inside a series (the core marks the point after one with gapBefore). */
export function gapRanges(points: HistoryPoint[]): [number, number][] {
  const out: [number, number][] = [];
  for (let i = 1; i < points.length; i++) {
    if (points[i]!.gapBefore) out.push([points[i - 1]!.t, points[i]!.t]);
  }
  return out;
}

/** Overlapping or touching intervals merged, sorted by start. */
export function mergeRanges(ranges: [number, number][]): [number, number][] {
  const sorted = [...ranges].sort((a, b) => a[0] - b[0]);
  const out: [number, number][] = [];
  for (const [a, b] of sorted) {
    const last = out[out.length - 1];
    if (last && a <= last[1]) last[1] = Math.max(last[1], b);
    else out.push([a, b]);
  }
  return out;
}

/** Times covered by both sets of intervals (each merged and sorted). */
export function intersectRanges(a: [number, number][], b: [number, number][]): [number, number][] {
  const out: [number, number][] = [];
  for (const [a0, a1] of a) {
    for (const [b0, b1] of b) {
      const lo = Math.max(a0, b0);
      const hi = Math.min(a1, b1);
      if (hi > lo) out.push([lo, hi]);
    }
  }
  return mergeRanges(out);
}

/** Spans where no series has data: the intersection of every series' gaps. */
export function commonGaps(seriesPoints: HistoryPoint[][]): [number, number][] {
  if (seriesPoints.length === 0) return [];
  return seriesPoints.map((pts) => mergeRanges(gapRanges(pts))).reduce((acc, g) => intersectRanges(acc, g));
}

/** The point nearest to `t` (points sorted by time), if one lies within `maxMs`; otherwise null. */
export function pointAt(points: HistoryPoint[], t: number, maxMs: number): HistoryPoint | null {
  let lo = 0;
  let hi = points.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (points[mid]!.t < t) lo = mid + 1;
    else hi = mid;
  }
  let best: HistoryPoint | null = null;
  for (const p of [points[lo - 1], points[lo]]) {
    if (p && Math.abs(p.t - t) <= maxMs && (best === null || Math.abs(p.t - t) < Math.abs(best.t - t))) best = p;
  }
  return best;
}

/** Evenly spaced time ticks aligned to whole hours or days. */
export function timeTicks(t0: number, t1: number, maxTicks: number): number[] {
  const HOUR = 3_600_000;
  const steps = [HOUR, 3 * HOUR, 6 * HOUR, 12 * HOUR, 24 * HOUR, 2 * 24 * HOUR, 7 * 24 * HOUR];
  const span = Math.max(1, t1 - t0);
  const step = steps.find((s) => span / s <= maxTicks) ?? steps[steps.length - 1]!;
  const tzOffset = new Date(t0).getTimezoneOffset() * 60_000;
  const first = Math.ceil((t0 - tzOffset) / step) * step + tzOffset;
  const out: number[] = [];
  for (let t = first; t <= t1; t += step) out.push(t);
  return out;
}
