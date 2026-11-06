// Maps a bucket's windows to the normalized 5-hour / weekly view by duration only.
// `primary` / `secondary` are positions, not meanings: the protocol does not promise that
// primary is the 5-hour window, so position is never used for classification.

import type { QuotaWindow, RateLimitBucket, WindowClass } from "../types.ts";

export type KnownWindowClass = Exclude<WindowClass, "unknown">;

/** Durations observed on codex-cli 0.162.0-alpha.2. Extend here when new windows appear. */
export const DEFAULT_WINDOW_CLASSES: ReadonlyMap<number, KnownWindowClass> = new Map([
  [300, "five-hour"],
  [10080, "weekly"],
]);

export function classifyWindow(window: QuotaWindow, table: ReadonlyMap<number, KnownWindowClass> = DEFAULT_WINDOW_CLASSES): WindowClass {
  if (window.windowDurationMins === null) return "unknown";
  return table.get(window.windowDurationMins) ?? "unknown";
}

export type WindowSlot = "primary" | "secondary";

export interface NormalizedWindows {
  fiveHour: QuotaWindow | null;
  weekly: QuotaWindow | null;
  /** Windows whose duration matched no known class; kept, never force-classified. */
  unknown: Array<{ slot: WindowSlot; window: QuotaWindow }>;
  warnings: string[];
}

export function normalizeCodexWindows(bucket: RateLimitBucket | null, table: ReadonlyMap<number, KnownWindowClass> = DEFAULT_WINDOW_CLASSES): NormalizedWindows {
  const result: NormalizedWindows = { fiveHour: null, weekly: null, unknown: [], warnings: [] };
  if (!bucket) return result;

  const found: Record<KnownWindowClass, Array<{ slot: WindowSlot; window: QuotaWindow }>> = { "five-hour": [], weekly: [] };
  const slots: Array<[WindowSlot, QuotaWindow | null]> = [
    ["primary", bucket.primary],
    ["secondary", bucket.secondary],
  ];
  for (const [slot, window] of slots) {
    if (!window) continue;
    const cls = classifyWindow(window, table);
    if (cls === "unknown") {
      result.unknown.push({ slot, window });
      result.warnings.push(`bucket ${bucket.limitId} ${slot}: windowDurationMins=${window.windowDurationMins} has no known class; kept as unknown`);
    } else {
      found[cls].push({ slot, window });
    }
  }

  for (const cls of ["five-hour", "weekly"] as const) {
    const hits = found[cls];
    if (hits.length > 1) {
      // Two windows claim the same class: picking one would be arbitrary, so neither is used.
      result.warnings.push(`bucket ${bucket.limitId}: ${hits.length} windows classify as ${cls} (${hits.map((h) => h.slot).join(", ")}); ${cls} left null`);
      continue;
    }
    const only = hits[0]?.window ?? null;
    if (cls === "five-hour") result.fiveHour = only;
    else result.weekly = only;
  }
  return result;
}
