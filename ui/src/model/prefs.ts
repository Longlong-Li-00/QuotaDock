// Page conveniences remembered in the window's local storage (never settings, never the backend).
// Storage can be unavailable or hold an outdated value: both fall back to the default.

import type { HistoryRange } from "../api/types.ts";

export const HISTORY_RANGES: HistoryRange[] = ["six_hours", "twelve_hours", "day", "week", "month", "current_cycle"];
export const DEFAULT_HISTORY_RANGE: HistoryRange = "twelve_hours";
const HISTORY_RANGE_KEY = "quotadock.history.range";

type KeyValueStore = Pick<Storage, "getItem" | "setItem">;

function storage(): KeyValueStore | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/** The range chosen last time, or the default (12 hours). */
export function loadHistoryRange(store: KeyValueStore | null = storage()): HistoryRange {
  try {
    const v = store?.getItem(HISTORY_RANGE_KEY);
    return HISTORY_RANGES.find((r) => r === v) ?? DEFAULT_HISTORY_RANGE;
  } catch {
    return DEFAULT_HISTORY_RANGE;
  }
}

export function saveHistoryRange(range: HistoryRange, store: KeyValueStore | null = storage()): void {
  try {
    store?.setItem(HISTORY_RANGE_KEY, range);
  } catch {
    // Not remembered this time; the page still works.
  }
}
