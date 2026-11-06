// Detects what changed between two snapshots. Phase 1B deliberately stops at detection:
// no change is classified as a scheduled, promotional, paid or global reset here.

import type { CodexQuotaSnapshot, QuotaTransition, QuotaTransitionChange, QuotaWindow } from "./types.ts";

type Slot = "primary" | "secondary";
type ComparableBuckets = Record<string, Record<Slot, QuotaWindow | null>>;

/** Buckets keyed by limitId; for legacy-only responses the normalized view stands in. */
function comparable(s: CodexQuotaSnapshot): ComparableBuckets {
  const ids = Object.keys(s.buckets);
  if (ids.length > 0) {
    return Object.fromEntries(ids.map((id) => [id, { primary: s.buckets[id]!.primary, secondary: s.buckets[id]!.secondary }]));
  }
  return { "(normalized)": { primary: s.fiveHour, secondary: s.weekly } };
}

export function detectQuotaTransition(prev: CodexQuotaSnapshot | null, next: CodexQuotaSnapshot): QuotaTransition & { availabilityChanged: boolean } {
  const result = {
    percentageChanged: false,
    resetTimestampChanged: false,
    bucketTopologyChanged: false,
    resetCandidate: false,
    availabilityChanged: false,
    changes: [] as QuotaTransitionChange[],
  };
  if (!prev) return result;

  result.availabilityChanged = prev.ordinaryUsageAllowed !== next.ordinaryUsageAllowed;
  const a = comparable(prev);
  const b = comparable(next);

  for (const id of new Set([...Object.keys(a), ...Object.keys(b)])) {
    const pa = a[id];
    const pb = b[id];
    if (!pa || !pb) {
      result.bucketTopologyChanged = true;
      result.changes.push({ bucket: id, slot: "bucket", field: "presence", from: Boolean(pa), to: Boolean(pb) });
      continue;
    }
    for (const slot of ["primary", "secondary"] as const) {
      const wa = pa[slot];
      const wb = pb[slot];
      if (!wa || !wb) {
        if (wa !== wb) {
          result.bucketTopologyChanged = true;
          result.changes.push({ bucket: id, slot, field: "presence", from: Boolean(wa), to: Boolean(wb) });
        }
        continue;
      }
      if (wa.windowDurationMins !== wb.windowDurationMins) {
        result.bucketTopologyChanged = true;
        result.changes.push({ bucket: id, slot, field: "windowDurationMins", from: wa.windowDurationMins, to: wb.windowDurationMins });
      }
      if (wa.usedPercent !== wb.usedPercent) {
        result.percentageChanged = true;
        if (wa.usedPercent !== null && wb.usedPercent !== null && wb.usedPercent < wa.usedPercent) result.resetCandidate = true;
        result.changes.push({ bucket: id, slot, field: "usedPercent", from: wa.usedPercent, to: wb.usedPercent });
      }
      if (wa.resetsAt !== wb.resetsAt) {
        result.resetTimestampChanged = true;
        result.changes.push({ bucket: id, slot, field: "resetsAt", from: wa.resetsAt, to: wb.resetsAt });
      }
    }
  }
  return result;
}
