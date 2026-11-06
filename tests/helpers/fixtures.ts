import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const fixturesDir = new URL("../fixtures/", import.meta.url);

export const realFixturePath = fileURLToPath(new URL("codex-rate-limits-real-redacted.json", fixturesDir));
export const fakeAppServerPath = fileURLToPath(new URL("./fakeAppServer.mjs", import.meta.url));

export function loadCase(name: string): unknown {
  const all = JSON.parse(readFileSync(new URL("codex-rate-limits.json", fixturesDir), "utf8")) as Record<string, unknown>;
  if (!(name in all)) throw new Error(`no fixture case ${name}`);
  return structuredClone(all[name]);
}

export function loadReal(): { _meta: Record<string, unknown>; response: unknown } {
  return JSON.parse(readFileSync(realFixturePath, "utf8"));
}
