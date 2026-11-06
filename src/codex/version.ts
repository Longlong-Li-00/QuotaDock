// Semver-style comparison for codex-cli versions such as "0.162.0-alpha.2" or "0.154.0-alpha.6.2".

import { VERIFIED_CODEX_VERSION, type VersionGate } from "./types.ts";

interface ParsedVersion {
  core: [number, number, number];
  prerelease: string[];
}

export function parseVersion(text: string): ParsedVersion | null {
  const m = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(text.trim());
  if (!m) return null;
  return {
    core: [Number(m[1]), Number(m[2]), Number(m[3])],
    prerelease: m[4] ? m[4].split(".") : [],
  };
}

/** Extracts "0.162.0-alpha.2" from "codex-cli 0.162.0-alpha.2". */
export function parseCodexVersionOutput(output: string): string | null {
  const m = /codex-cli\s+(\S+)/.exec(output);
  return m?.[1] && parseVersion(m[1]) ? m[1] : null;
}

function compareIdentifiers(a: string, b: string): number {
  const an = /^\d+$/.test(a);
  const bn = /^\d+$/.test(b);
  if (an && bn) return Math.sign(Number(a) - Number(b));
  if (an) return -1; // numeric identifiers sort before alphanumeric ones
  if (bn) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Returns <0, 0 or >0. Throws if either version is unparseable. */
export function compareVersions(a: string, b: string): number {
  const pa = parseVersion(a);
  const pb = parseVersion(b);
  if (!pa || !pb) throw new Error(`cannot compare versions "${a}" and "${b}"`);
  for (let i = 0; i < 3; i++) {
    const d = pa.core[i]! - pb.core[i]!;
    if (d !== 0) return Math.sign(d);
  }
  // A version without a pre-release tag ranks above any pre-release of the same core.
  if (pa.prerelease.length === 0 || pb.prerelease.length === 0) {
    return pb.prerelease.length - pa.prerelease.length === 0 ? 0 : pa.prerelease.length === 0 ? 1 : -1;
  }
  const n = Math.max(pa.prerelease.length, pb.prerelease.length);
  for (let i = 0; i < n; i++) {
    const x = pa.prerelease[i];
    const y = pb.prerelease[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    const d = compareIdentifiers(x, y);
    if (d !== 0) return d;
  }
  return 0;
}

export function gateVersion(version: string, verified = VERIFIED_CODEX_VERSION): VersionGate {
  if (!parseVersion(version)) return "unparseable";
  const d = compareVersions(version, verified);
  if (d === 0) return "verified";
  return d > 0 ? "unverified-newer" : "unsupported-older";
}
