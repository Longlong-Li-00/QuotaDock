// Turns a raw `account/rateLimits/read` result into a CodexQuotaSnapshot.

import type { CodexQuotaSnapshot, RateLimitBucket, TransportKind, UsageAvailability } from "../types.ts";
import { normalizeCodexWindows } from "./normalizeWindows.ts";
import { parseRateLimitsResponse, type ParsedRateLimits } from "./parseRateLimits.ts";

export const CODEX_LIMIT_ID = "codex";

/** ordinaryUsageAllowed is the only availability signal; percentages and reset times are never consulted. */
export function toAvailability(ordinaryUsageAllowed: boolean | null | undefined): UsageAvailability {
  if (ordinaryUsageAllowed === true) return "allowed";
  if (ordinaryUsageAllowed === false) return "blocked";
  return "unknown";
}

export interface BucketSelection {
  bucket: RateLimitBucket | null;
  source: "codex" | "legacy" | null;
  warnings: string[];
}

/**
 * Picks the bucket that feeds the normalized Codex view:
 *   1. rateLimitsByLimitId["codex"]
 *   2. legacy `rateLimits` when rateLimitsByLimitId is absent (or lacks "codex" but the legacy
 *      view says it is "codex")
 * Never "the first bucket": map order says nothing about which bucket is Codex.
 */
export function selectCodexBucket(parsed: ParsedRateLimits): BucketSelection {
  const { byLimitId, legacy } = parsed;
  if (byLimitId) {
    const codex = byLimitId[CODEX_LIMIT_ID];
    if (codex) return { bucket: codex, source: "codex", warnings: [] };
    if (legacy && legacy.limitId === CODEX_LIMIT_ID) {
      return { bucket: legacy, source: "legacy", warnings: ['rateLimitsByLimitId has no "codex" bucket; normalized view taken from legacy rateLimits (limitId=codex)'] };
    }
    const keys = Object.keys(byLimitId);
    return { bucket: null, source: null, warnings: [`no "codex" bucket (buckets: ${keys.length ? keys.join(", ") : "none"}); normalized view left empty`] };
  }
  if (legacy) {
    return { bucket: legacy, source: "legacy", warnings: [`rateLimitsByLimitId absent; normalized view taken from legacy rateLimits (limitId=${legacy.limitId})`] };
  }
  return { bucket: null, source: null, warnings: ["response contains neither rateLimitsByLimitId nor rateLimits"] };
}

export interface SnapshotContext {
  capturedAt: string;
  codexCliVersion: string | null;
  transport: TransportKind;
  includeRaw?: boolean;
}

export function buildCodexSnapshot(raw: unknown, ctx: SnapshotContext): CodexQuotaSnapshot {
  const parsed = parseRateLimitsResponse(raw);
  const selection = selectCodexBucket(parsed);
  const normalized = normalizeCodexWindows(selection.bucket);
  const snapshot: CodexQuotaSnapshot = {
    provider: "codex",
    capturedAt: ctx.capturedAt,
    codexCliVersion: ctx.codexCliVersion,
    transport: ctx.transport,
    ordinaryUsageAllowed: parsed.ordinaryUsageAllowed,
    availability: toAvailability(parsed.ordinaryUsageAllowed),
    fiveHour: normalized.fiveHour,
    weekly: normalized.weekly,
    sourceBucket: selection.source,
    buckets: parsed.byLimitId ?? {},
    resetCredits: parsed.resetCredits,
    accountId: parsed.accountId,
    warnings: [...parsed.warnings, ...selection.warnings, ...normalized.warnings],
  };
  if (ctx.includeRaw) snapshot.raw = raw;
  return snapshot;
}
