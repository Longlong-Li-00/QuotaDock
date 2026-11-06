// Single entry point for scrubbing anything that may reach a log, the terminal or a fixture.

const REDACTED = "[REDACTED]";

// Keys whose values are credentials, wherever they appear.
const CREDENTIAL_KEY = /token|secret|password|passwd|cookie|authorization|api[-_]?key|session[-_]?id|bearer/i;

// Keys that identify the person, account or device. Matched exactly (case-insensitive) so that
// harmless ids such as `limitId` or JSON-RPC `id` survive.
const IDENTITY_KEYS = new Set(
  [
    "email",
    "accountId",
    "account_id",
    "chatgptAccountId",
    "chatgpt_account_id",
    "userId",
    "user_id",
    "orgId",
    "org_id",
    "organizationId",
    "workspaceId",
    "installationId",
    "installation_id",
    "serverName",
  ].map((k) => k.toLowerCase()),
);

const JWT = /\beyJ[\w-]{5,}\.[\w-]{5,}\.[\w-]*/g;
const BEARER = /\bBearer\s+[\w.~+/=-]+/gi;
const EMAIL = /[\w.+-]+@[\w-]+(\.[\w-]+)+/g;
const SK_KEY = /\bsk-[\w-]{10,}/g;

export function sanitizeString(value: string): string {
  return value
    .replace(JWT, "[REDACTED_JWT]")
    .replace(BEARER, "Bearer [REDACTED]")
    .replace(SK_KEY, "[REDACTED_KEY]")
    .replace(EMAIL, "[REDACTED_EMAIL]");
}

export function isSensitiveKey(key: string): boolean {
  return CREDENTIAL_KEY.test(key) || IDENTITY_KEYS.has(key.toLowerCase());
}

/**
 * Deep copy of `value` with credentials and identifiers removed. Safe to call on anything:
 * strings are scanned for JWTs, bearer headers, API keys and e-mail addresses.
 */
export function sanitizeForLog(value: unknown, depth = 0): unknown {
  if (depth > 32) return "[TRUNCATED]";
  if (typeof value === "string") return sanitizeString(value);
  if (value === null || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map((v) => sanitizeForLog(v, depth + 1));
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(value)) {
    out[k] = isSensitiveKey(k) && v !== null && v !== undefined ? REDACTED : sanitizeForLog(v, depth + 1);
  }
  return out;
}

/** Partial mask for display: keeps a short prefix/suffix so two ids can be told apart. */
export function maskIdentifier(id: string | null): string | null {
  if (id === null) return null;
  if (id.length <= 8) return "****";
  return `${id.slice(0, 4)}****${id.slice(-2)}`;
}
