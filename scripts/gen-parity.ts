// Generates the TypeScript reference output that the Rust normalizer must reproduce.
//   node scripts/gen-parity.ts   ->   tests/fixtures/parity/ts-normalized.json
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { buildCodexSnapshot } from "../src/codex/parser/buildSnapshot.ts";

const cases = JSON.parse(readFileSync(new URL("../tests/fixtures/codex-rate-limits.json", import.meta.url), "utf8")) as Record<string, unknown>;
const real = JSON.parse(readFileSync(new URL("../tests/fixtures/codex-rate-limits-real-redacted.json", import.meta.url), "utf8")) as { response: unknown };
const inputs: Record<string, unknown> = { real_redacted: real.response };
for (const [k, v] of Object.entries(cases)) if (!k.startsWith("_")) inputs[k] = v;

const out: Record<string, unknown> = {};
for (const [name, raw] of Object.entries(inputs)) {
  const s = buildCodexSnapshot(raw, { capturedAt: "1970-01-01T00:00:00.000Z", codexCliVersion: null, transport: "stdio-app-server" });
  out[name] = {
    ordinaryUsageAllowed: s.ordinaryUsageAllowed,
    availability: s.availability,
    fiveHour: s.fiveHour,
    weekly: s.weekly,
    sourceBucket: s.sourceBucket,
    buckets: s.buckets,
    resetCredits: s.resetCredits,
    accountId: s.accountId,
    warnings: s.warnings,
  };
}
mkdirSync(new URL("../tests/fixtures/parity/", import.meta.url), { recursive: true });
writeFileSync(new URL("../tests/fixtures/parity/ts-normalized.json", import.meta.url), JSON.stringify(out, null, 2) + "\n");
console.log(`wrote ${Object.keys(out).length} cases`);
