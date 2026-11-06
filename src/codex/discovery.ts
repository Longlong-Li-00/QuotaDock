// Locates codex executables on this machine without hard-coding versioned install paths,
// and checks whether a managed app-server daemon is running.

import { execFile } from "node:child_process";
import { existsSync, readdirSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { sanitizeString } from "./sanitize.ts";
import type { CodexInstallation, CodexInstallationSource, DaemonStatus, VersionGate } from "./types.ts";
import { compareVersions, gateVersion, parseCodexVersionOutput } from "./version.ts";

export interface CapturedRun {
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
  spawnError?: string;
}

/** Runs a short-lived command without a shell and captures its output. */
export function runCapture(file: string, args: string[], timeoutMs = 15_000, env?: NodeJS.ProcessEnv): Promise<CapturedRun> {
  return new Promise((resolve) => {
    execFile(
      file,
      args,
      { timeout: timeoutMs, windowsHide: true, encoding: "utf8", maxBuffer: 4 * 1024 * 1024, env: env ?? process.env },
      (error, stdout, stderr) => {
        const err = error as (NodeJS.ErrnoException & { killed?: boolean; code?: unknown }) | null;
        resolve({
          code: err ? (typeof err.code === "number" ? err.code : null) : 0,
          stdout: String(stdout ?? ""),
          stderr: String(stderr ?? ""),
          timedOut: Boolean(err?.killed),
          spawnError: err && typeof err.code === "string" ? `${err.code}: ${err.message}` : undefined,
        });
      },
    );
  });
}

export interface CandidatePath {
  path: string;
  source: CodexInstallationSource;
  detail: string;
}

export interface DiscoveredInstallation extends CodexInstallation {
  gate: VersionGate;
}

export interface DiscoveryResult {
  candidates: DiscoveredInstallation[];
  rejected: Array<{ path: string; source: CodexInstallationSource; reason: string }>;
  selected: DiscoveredInstallation | null;
  selectionReason: string;
}

export interface DiscoveryOptions {
  /** Explicit executable (future Settings field); also read from QUOTADOCK_CODEX_PATH. */
  manualPath?: string;
  allowUnverifiedVersion?: boolean;
}

const SOURCE_PRIORITY: Record<CodexInstallationSource, number> = {
  manual: 0,
  desktop: 1,
  standalone: 2,
  vscode: 3,
  unknown: 4,
};

/** Codex Desktop from the Microsoft Store: ask Windows where the package lives instead of guessing. */
async function desktopPackageCandidates(): Promise<CandidatePath[]> {
  if (process.platform !== "win32") return [];
  const run = await runCapture("powershell.exe", [
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    "Get-AppxPackage -Name OpenAI.Codex | ForEach-Object { $_.InstallLocation }",
  ]);
  return run.stdout
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
    .map((loc) => ({
      path: join(loc, "app", "resources", "codex.exe"),
      source: "desktop" as const,
      detail: "Codex Desktop package (Get-AppxPackage OpenAI.Codex)",
    }));
}

/** Copies Codex Desktop stages under %LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\ and runs from there. */
function desktopRuntimeCandidates(): CandidatePath[] {
  const base = process.env.LOCALAPPDATA ? join(process.env.LOCALAPPDATA, "OpenAI", "Codex", "bin") : null;
  if (!base || !existsSync(base)) return [];
  return safeReaddir(base)
    .map((d) => join(base, d, "codex.exe"))
    .filter((p) => existsSync(p))
    .map((p) => ({ path: p, source: "desktop" as const, detail: "runtime copy staged by Codex Desktop" }));
}

/** A standalone CLI on PATH (`npm i -g @openai/codex` or a native install). Not present on the reference machine. */
async function standaloneCandidates(): Promise<CandidatePath[]> {
  const out: CandidatePath[] = [];
  const which = process.platform === "win32" ? await runCapture("where.exe", ["codex"]) : await runCapture("which", ["-a", "codex"]);
  for (const line of which.stdout.split(/\r?\n/).map((l) => l.trim()).filter(Boolean)) {
    if (/\.exe$/i.test(line) || process.platform !== "win32") {
      out.push({ path: line, source: "standalone", detail: "codex on PATH" });
    }
  }
  // npm shims (codex.cmd / codex.ps1) wrap a native binary; use the binary directly so no shell is needed.
  const npmRoot = process.env.APPDATA ? join(process.env.APPDATA, "npm", "node_modules", "@openai", "codex", "vendor") : null;
  if (npmRoot && existsSync(npmRoot)) {
    for (const target of safeReaddir(npmRoot)) {
      const p = join(npmRoot, target, "codex", "codex.exe");
      if (existsSync(p)) out.push({ path: p, source: "standalone", detail: `npm global @openai/codex (${target})` });
    }
  }
  return out;
}

function vscodeCandidates(): CandidatePath[] {
  const out: CandidatePath[] = [];
  const editors = [".vscode", ".vscode-insiders", ".cursor", ".windsurf"];
  const platformDir = process.platform === "win32" ? "windows-x86_64" : process.platform === "darwin" ? "macos-aarch64" : "linux-x86_64";
  const exe = process.platform === "win32" ? "codex.exe" : "codex";
  for (const editor of editors) {
    const extRoot = join(homedir(), editor, "extensions");
    if (!existsSync(extRoot)) continue;
    for (const dir of safeReaddir(extRoot).filter((d) => d.startsWith("openai.chatgpt-"))) {
      const p = join(extRoot, dir, "bin", platformDir, exe);
      if (existsSync(p)) out.push({ path: p, source: "vscode", detail: `${editor} extension ${dir}` });
    }
  }
  return out;
}

function safeReaddir(dir: string): string[] {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

function canonical(p: string): string {
  try {
    return realpathSync.native(p).toLowerCase();
  } catch {
    return p.toLowerCase();
  }
}

export async function listCandidatePaths(options: DiscoveryOptions = {}): Promise<CandidatePath[]> {
  const manual = options.manualPath ?? process.env.QUOTADOCK_CODEX_PATH;
  const lists = await Promise.all([desktopPackageCandidates(), standaloneCandidates()]);
  const all: CandidatePath[] = [
    ...(manual ? [{ path: manual, source: "manual" as const, detail: "explicitly configured" }] : []),
    ...lists[0],
    ...desktopRuntimeCandidates(),
    ...lists[1],
    ...vscodeCandidates(),
  ];
  const seen = new Set<string>();
  return all.filter((c) => {
    const key = canonical(c.path);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/**
 * Picks the executable to run. Manual configuration wins; otherwise the highest acceptable
 * version, ties broken by source (desktop > standalone > vscode).
 */
export function selectInstallation(candidates: DiscoveredInstallation[], allowUnverifiedVersion = false): { selected: DiscoveredInstallation | null; reason: string } {
  const manual = candidates.find((c) => c.source === "manual");
  if (manual) {
    const ok = manual.gate === "verified" || manual.gate === "unverified-newer" || allowUnverifiedVersion;
    return ok
      ? { selected: manual, reason: "manually configured executable" }
      : { selected: null, reason: `manually configured executable has ${manual.gate} version ${manual.version}` };
  }
  const acceptable = candidates.filter((c) => c.gate === "verified" || c.gate === "unverified-newer" || (allowUnverifiedVersion && c.gate === "unsupported-older"));
  if (acceptable.length === 0) {
    return { selected: null, reason: candidates.length === 0 ? "no codex executable found" : "no executable with a verified or newer version" };
  }
  const sorted = [...acceptable].sort((a, b) => compareVersions(b.version, a.version) || SOURCE_PRIORITY[a.source] - SOURCE_PRIORITY[b.source]);
  const best = sorted[0]!;
  return { selected: best, reason: `highest acceptable version (${best.version}, ${best.gate}) from ${best.source}` };
}

export async function discoverCodexInstallations(options: DiscoveryOptions = {}): Promise<DiscoveryResult> {
  const paths = await listCandidatePaths(options);
  const candidates: DiscoveredInstallation[] = [];
  const rejected: DiscoveryResult["rejected"] = [];
  await Promise.all(
    paths.map(async (c) => {
      if (!existsSync(c.path)) {
        rejected.push({ path: c.path, source: c.source, reason: "file does not exist" });
        return;
      }
      const run = await runCapture(c.path, ["--version"], 15_000);
      const version = parseCodexVersionOutput(run.stdout);
      if (!version) {
        rejected.push({ path: c.path, source: c.source, reason: `--version failed: ${sanitizeString((run.spawnError ?? run.stderr).split(/\r?\n/)[0] ?? "")}` });
        return;
      }
      candidates.push({ path: c.path, version, source: c.source, detail: c.detail, gate: gateVersion(version) });
    }),
  );
  // Keep a stable, readable order: by source priority, then version descending.
  candidates.sort((a, b) => SOURCE_PRIORITY[a.source] - SOURCE_PRIORITY[b.source] || compareVersions(b.version, a.version));
  const { selected, reason } = selectInstallation(candidates, options.allowUnverifiedVersion);
  return { candidates, rejected, selected, selectionReason: reason };
}

// ---------------------------------------------------------------------------
// Managed daemon
// ---------------------------------------------------------------------------

/**
 * `codex app-server daemon version` prints CLI and running app-server versions as JSON when a
 * managed daemon is up, and fails to connect to the control socket otherwise. A running Codex
 * Desktop does NOT imply a daemon: Desktop runs its own private stdio app-server child.
 */
export async function getDaemonStatus(codexPath: string, env?: NodeJS.ProcessEnv): Promise<DaemonStatus> {
  const run = await runCapture(codexPath, ["app-server", "daemon", "version"], 15_000, env);
  const firstErrorLine = sanitizeString((run.spawnError ?? run.stderr).split(/\r?\n/).find((l) => l.trim()) ?? "").trim();
  if (run.code === 0) {
    let parsed: unknown = null;
    try {
      parsed = JSON.parse(run.stdout);
    } catch {
      return { available: true, running: true, error: "daemon answered but output was not JSON", raw: sanitizeString(run.stdout.slice(0, 2000)) };
    }
    return { available: true, running: true, ...extractDaemonFields(parsed), raw: parsed };
  }
  if (/unrecognized subcommand|unexpected argument/i.test(run.stderr)) {
    return { available: false, running: false, error: firstErrorLine };
  }
  return { available: true, running: false, error: firstErrorLine || (run.timedOut ? "timed out" : `exit code ${run.code}`) };
}

/**
 * Lifecycle JSON is camelCase `{ status, backend?, socketPath, cliVersion?, appServerVersion? }`
 * [OPENAI CODEX SOURCE app-server-daemon/src/lib.rs @ rust-v0.162.0-alpha.2].
 */
export function extractDaemonFields(json: unknown): { version?: string; backend?: string } {
  if (!json || typeof json !== "object") return {};
  const o = json as Record<string, unknown>;
  const str = (v: unknown): string | undefined => (typeof v === "string" && v ? v : undefined);
  return { version: str(o.appServerVersion), backend: str(o.backend) };
}
