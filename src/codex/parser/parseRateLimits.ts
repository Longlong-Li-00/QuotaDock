// Parses an `account/rateLimits/read` result (schema: v2/GetAccountRateLimitsResponse).
// Defensive by design: unexpected shapes become nulls plus warnings, never invented values.

import type { QuotaWindow, RateLimitBucket } from "../types.ts";

export interface ParsedRateLimits {
  ordinaryUsageAllowed: boolean | null;
  accountId: string | null;
  /** `rateLimits`: the backward-compatible single-bucket view. */
  legacy: RateLimitBucket | null;
  /** `rateLimitsByLimitId`, every bucket kept; null when the field is absent or null. */
  byLimitId: Record<string, RateLimitBucket> | null;
  resetCredits: { availableCount: number | null } | null;
  warnings: string[];
}

export class RateLimitParseError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RateLimitParseError";
  }
}

// Unix-seconds sanity range (2001 .. ~5138). Anything outside is kept raw but not rendered,
// because it would mean the unit is not what was observed (seconds).
const MIN_PLAUSIBLE_SECONDS = 1_000_000_000;
const MAX_PLAUSIBLE_SECONDS = 100_000_000_000;

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

function finiteNumber(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

export function computeRemainingPercent(usedPercent: number | null): number | null {
  return usedPercent === null ? null : Math.max(0, Math.min(100, 100 - usedPercent));
}

export function resetsAtToIso(resetsAt: number | null, warnings?: string[], where = "window"): string | null {
  if (resetsAt === null) return null;
  if (resetsAt < MIN_PLAUSIBLE_SECONDS || resetsAt > MAX_PLAUSIBLE_SECONDS) {
    warnings?.push(`${where}: resetsAt=${resetsAt} is outside the plausible Unix-seconds range; kept raw, not converted`);
    return null;
  }
  return new Date(resetsAt * 1000).toISOString();
}

export function parseWindow(raw: unknown, where: string, warnings: string[]): QuotaWindow | null {
  if (raw === null || raw === undefined) return null;
  if (!isRecord(raw)) {
    warnings.push(`${where}: window is not an object; ignored`);
    return null;
  }
  const usedPercent = finiteNumber(raw.usedPercent);
  if (usedPercent === null && raw.usedPercent !== undefined && raw.usedPercent !== null) {
    warnings.push(`${where}: usedPercent is not a number`);
  }
  if (usedPercent !== null && (usedPercent < 0 || usedPercent > 100)) {
    warnings.push(`${where}: usedPercent=${usedPercent} is outside 0..100; kept as reported`);
  }
  const windowDurationMins = finiteNumber(raw.windowDurationMins);
  const resetsAt = finiteNumber(raw.resetsAt);
  return {
    usedPercent,
    remainingPercent: computeRemainingPercent(usedPercent),
    windowDurationMins,
    resetsAt,
    resetsAtIso: resetsAtToIso(resetsAt, warnings, where),
  };
}

export function parseBucket(raw: unknown, limitId: string, where: string, warnings: string[]): RateLimitBucket | null {
  if (raw === null || raw === undefined) return null;
  if (!isRecord(raw)) {
    warnings.push(`${where}: bucket is not an object; ignored`);
    return null;
  }
  return {
    limitId,
    primary: parseWindow(raw.primary, `${where}.primary`, warnings),
    secondary: parseWindow(raw.secondary, `${where}.secondary`, warnings),
    raw,
  };
}

export function parseRateLimitsResponse(raw: unknown): ParsedRateLimits {
  if (!isRecord(raw)) throw new RateLimitParseError("account/rateLimits/read result is not an object");
  const warnings: string[] = [];

  let ordinaryUsageAllowed: boolean | null = null;
  if (typeof raw.ordinaryUsageAllowed === "boolean") ordinaryUsageAllowed = raw.ordinaryUsageAllowed;
  else if (raw.ordinaryUsageAllowed !== null && raw.ordinaryUsageAllowed !== undefined) {
    warnings.push("ordinaryUsageAllowed is not a boolean; treated as unknown");
  }

  const legacyRaw = raw.rateLimits;
  const legacyId = isRecord(legacyRaw) && typeof legacyRaw.limitId === "string" ? legacyRaw.limitId : "legacy";
  const legacy = parseBucket(legacyRaw, legacyId, "rateLimits", warnings);

  let byLimitId: Record<string, RateLimitBucket> | null = null;
  if (isRecord(raw.rateLimitsByLimitId)) {
    byLimitId = {};
    for (const [key, value] of Object.entries(raw.rateLimitsByLimitId)) {
      const where = `rateLimitsByLimitId.${key}`;
      if (isRecord(value) && typeof value.limitId === "string" && value.limitId !== key) {
        warnings.push(`${where}: inner limitId "${value.limitId}" differs from its key; keyed by "${key}"`);
      }
      const bucket = parseBucket(value, key, where, warnings);
      if (bucket) byLimitId[key] = bucket;
    }
  } else if (raw.rateLimitsByLimitId !== null && raw.rateLimitsByLimitId !== undefined) {
    warnings.push("rateLimitsByLimitId is not an object; ignored");
  }

  let resetCredits: ParsedRateLimits["resetCredits"] = null;
  if (isRecord(raw.rateLimitResetCredits)) {
    const count = finiteNumber(raw.rateLimitResetCredits.availableCount);
    if (count === null) warnings.push("rateLimitResetCredits.availableCount is not a number");
    resetCredits = { availableCount: count };
  }

  return {
    ordinaryUsageAllowed,
    accountId: typeof raw.accountId === "string" ? raw.accountId : null,
    legacy,
    byLimitId,
    resetCredits,
    warnings,
  };
}
