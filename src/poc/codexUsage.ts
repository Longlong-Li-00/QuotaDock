// QuotaDock Phase 1B PoC CLI.
//
//   npm run poc:codex                          one read, snapshot JSON on stdout
//   npm run poc:codex -- --watch               poll every 5 min; Enter = manual refresh, q = quit
//   npm run poc:codex -- --watch --interval 60 development interval (>= 30 s)
//   npm run poc:codex -- --discover            list executables + daemon status, no RPC
//   npm run poc:codex -- --save-fixture tests/fixtures/codex-rate-limits-real-redacted.json
//
// stdout carries JSON only; diagnostics go to stderr. Use `npm run -s` to drop npm's banner.

import { writeFileSync } from "node:fs";
import { createInterface } from "node:readline";
import { parseArgs } from "node:util";
import { discoverCodexInstallations, getDaemonStatus } from "../codex/discovery.ts";
import { PollingController, DEFAULT_INTERVAL_MS, validateIntervalMs } from "../codex/polling/pollingController.ts";
import { CodexProvider, type TransportPreference } from "../codex/provider/codexProvider.ts";
import { maskIdentifier, sanitizeForLog, sanitizeString } from "../codex/sanitize.ts";
import { detectQuotaTransition } from "../codex/transition.ts";
import type { CodexQuotaSnapshot, CodexReadResult, Logger, QuotaWindow } from "../codex/types.ts";

const HELP = `QuotaDock Codex provider PoC (read-only)

Options:
  --watch                    poll repeatedly (default every 300 s); Enter = refresh now, q = quit
  --interval <seconds>       poll interval for --watch (min 30; below 300 is development only)
  --unsafe-dev-poll          allow intervals down to 10 s (development only, never default)
  --transport <mode>         auto | daemon-proxy | stdio-app-server   (default auto)
  --codex-path <path>        use this codex executable instead of discovery
  --allow-unverified-version permit codex versions older than the verified one
  --raw                      include the sanitized raw RPC result
  --save-fixture <path>      write the redacted raw result as a test fixture
  --discover                 only list codex executables and daemon status
  --verbose                  debug logging on stderr
  -h, --help`;

const { values: args } = parseArgs({
  options: {
    watch: { type: "boolean", default: false },
    interval: { type: "string" },
    "unsafe-dev-poll": { type: "boolean", default: false },
    transport: { type: "string", default: "auto" },
    "codex-path": { type: "string" },
    "allow-unverified-version": { type: "boolean", default: false },
    raw: { type: "boolean", default: false },
    "save-fixture": { type: "string" },
    discover: { type: "boolean", default: false },
    verbose: { type: "boolean", default: false },
    help: { type: "boolean", short: "h", default: false },
  },
  strict: true,
});

const err = (line: string): void => {
  process.stderr.write(line + "\n");
};
const log: Logger = {
  info: (m) => err(`[quotadock] ${sanitizeString(m)}`),
  warn: (m) => err(`[quotadock] WARN ${sanitizeString(m)}`),
  debug: (m) => {
    if (args.verbose) err(`[quotadock] debug ${sanitizeString(m)}`);
  },
};

function fail(message: string): never {
  err(`[quotadock] ${message}`);
  process.exit(64);
}

if (args.help) {
  err(HELP);
  process.exit(0);
}
const transportPref = args.transport as TransportPreference;
if (!["auto", "daemon-proxy", "stdio-app-server"].includes(transportPref)) fail(`unknown --transport ${args.transport}`);

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

function toDisplay(snapshot: CodexQuotaSnapshot, includeRaw: boolean): unknown {
  const { raw, buckets, ...rest } = snapshot;
  const shown = {
    ...rest,
    buckets: Object.fromEntries(
      Object.entries(buckets).map(([id, b]) => [id, includeRaw ? b : { limitId: b.limitId, primary: b.primary, secondary: b.secondary }]),
    ),
    ...(includeRaw && raw !== undefined ? { raw } : {}),
  };
  const sanitized = sanitizeForLog(shown) as Record<string, unknown>;
  sanitized.accountId = maskIdentifier(snapshot.accountId); // partial mask for the terminal
  return sanitized;
}

function fmtTime(epochSeconds: number | null): string {
  if (epochSeconds === null) return "unknown";
  const d = new Date(epochSeconds * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  const local = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const deltaMin = Math.round((d.getTime() - Date.now()) / 60_000);
  const rel = deltaMin >= 0 ? `in ${Math.floor(deltaMin / 60)}h${pad(deltaMin % 60)}m` : `${-deltaMin} min ago`;
  return `${local} (${rel})`;
}

function fmtWindow(label: string, w: QuotaWindow | null): string {
  if (!w) return `${label}: n/a`;
  return `${label}: ${w.usedPercent ?? "?"}% used, ${w.remainingPercent ?? "?"}% left, resets ${fmtTime(w.resetsAt)}`;
}

function summarize(result: CodexReadResult): string {
  if (result.status !== "ok") return `${result.status}: ${result.message}`;
  const s = result.snapshot;
  return [`availability=${s.availability}`, fmtWindow("5h", s.fiveHour), fmtWindow("weekly", s.weekly), `buckets=${Object.keys(s.buckets).join(",") || "none"}`, `via ${s.transport}`].join(" | ");
}

function exitCodeFor(result: CodexReadResult): number {
  return result.status === "ok" ? 0 : result.status === "not-authenticated" ? 2 : 1;
}

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

async function runDiscover(): Promise<void> {
  const discovery = await discoverCodexInstallations({ manualPath: args["codex-path"], allowUnverifiedVersion: args["allow-unverified-version"] });
  const daemon = discovery.selected ? await getDaemonStatus(discovery.selected.path) : null;
  process.stdout.write(JSON.stringify(sanitizeForLog({ ...discovery, daemon }), null, 2) + "\n");
}

function reportEnvironment(provider: CodexProvider): void {
  const env = provider.environment;
  for (const c of env.discovery?.candidates ?? []) log.info(`found codex ${c.version} [${c.gate}] (${c.source}: ${c.detail ?? ""}) ${c.path}`);
  for (const r of env.discovery?.rejected ?? []) log.info(`rejected ${r.path}: ${r.reason}`);
  if (env.installation) log.info(`selected ${env.installation.path} (${env.discovery?.selectionReason})`);
  if (env.daemon) log.info(`daemon: available=${env.daemon.available} running=${env.daemon.running}${env.daemon.error ? ` (${env.daemon.error})` : ""}`);
  if (env.transport) log.info(`transport: ${env.transport} pid=${env.pid} - ${env.transportReason}`);
  if (env.appServerVersion) log.info(`app-server version: ${env.appServerVersion}`);
  if (env.account) log.info(`account: type=${env.account.type} plan=${env.account.planType} rate-limit accountId vs account/read: ${env.accountCheck}`);
}

async function runOnce(provider: CodexProvider): Promise<number> {
  const result = await provider.read();
  reportEnvironment(provider);
  log.info(summarize(result));
  if (result.status === "ok") {
    for (const w of result.snapshot.warnings) log.warn(w);
    process.stdout.write(JSON.stringify(toDisplay(result.snapshot, args.raw), null, 2) + "\n");
    if (args["save-fixture"]) {
      const fixture = {
        _meta: {
          description: "Real account/rateLimits/read result, redacted (account/user identifiers removed).",
          capturedAt: result.snapshot.capturedAt,
          codexCliVersion: result.snapshot.codexCliVersion,
          transport: result.snapshot.transport,
          params: { excludeResetCreditDetails: true, supportsLunaReserve: false },
        },
        response: sanitizeForLog(result.snapshot.raw),
      };
      writeFileSync(args["save-fixture"], JSON.stringify(fixture, null, 2) + "\n");
      log.info(`fixture written to ${args["save-fixture"]}`);
    }
  } else {
    process.stdout.write(JSON.stringify(sanitizeForLog(result), null, 2) + "\n");
  }
  return exitCodeFor(result);
}

async function runWatch(provider: CodexProvider): Promise<void> {
  let intervalMs = DEFAULT_INTERVAL_MS;
  if (args.interval !== undefined) {
    const check = validateIntervalMs(Number(args.interval) * 1000, args["unsafe-dev-poll"]);
    if (!check.ok) fail(check.error!);
    intervalMs = check.intervalMs;
    if (check.developmentOnly) log.warn(`interval ${intervalMs / 1000} s is DEVELOPMENT ONLY (default is ${DEFAULT_INTERVAL_MS / 1000} s); every read is a live backend request`);
  }
  log.info(`watch mode: every ${intervalMs / 1000} s. Enter = refresh now, q + Enter = quit.`);

  let previous: CodexQuotaSnapshot | null = null;
  let reportedEnv = false;
  provider.onNotification((n) => log.debug(`notification ${n.method}`));

  const controller = new PollingController<CodexReadResult>({
    tick: () => provider.read(),
    classify: (r) => (r.status === "ok" ? "success" : "failure"),
    intervalMs,
    onEvent: (event) => {
      if (!reportedEnv && provider.environment.transport) {
        reportEnvironment(provider);
        reportedEnv = true;
      }
      const next = event.nextDelayMs === null ? "stopped" : `next in ${Math.round(event.nextDelayMs / 1000)} s`;
      if (event.error !== undefined) {
        log.warn(`${event.trigger} read threw: ${event.error instanceof Error ? event.error.message : String(event.error)} (${next}, failures=${event.consecutiveFailures})`);
        return;
      }
      const result = event.result!;
      log.info(`[${new Date().toLocaleTimeString()}] ${event.trigger}: ${summarize(result)} (${next}${event.consecutiveFailures ? `, failures=${event.consecutiveFailures}` : ""})`);
      if (result.status !== "ok") {
        process.stdout.write(JSON.stringify({ type: "result", result: sanitizeForLog(result) }) + "\n");
        return;
      }
      const transition = detectQuotaTransition(previous, result.snapshot);
      if (transition.changes.length > 0 || transition.availabilityChanged) {
        log.info(`transition: percentageChanged=${transition.percentageChanged} resetTimestampChanged=${transition.resetTimestampChanged} bucketTopologyChanged=${transition.bucketTopologyChanged} resetCandidate=${transition.resetCandidate} availabilityChanged=${transition.availabilityChanged}`);
        for (const c of transition.changes) log.info(`  ${c.bucket}.${c.slot}.${c.field}: ${JSON.stringify(c.from)} -> ${JSON.stringify(c.to)}`);
      }
      previous = result.snapshot;
      process.stdout.write(JSON.stringify({ type: "snapshot", snapshot: toDisplay(result.snapshot, args.raw), transition }) + "\n");
    },
  });

  const shutdown = async (): Promise<void> => {
    controller.stop();
    await controller.idle();
    await provider.stop();
    log.info("stopped; app-server child closed");
    process.exit(0);
  };
  process.on("SIGINT", () => void shutdown());
  process.on("SIGTERM", () => void shutdown());

  const rl = createInterface({ input: process.stdin });
  rl.on("line", (line) => {
    if (line.trim().toLowerCase() === "q") {
      void shutdown();
      return;
    }
    const r = controller.refreshNow();
    log.info(`manual refresh: ${r}`);
  });

  controller.start();
}

async function main(): Promise<void> {
  if (args.discover) {
    await runDiscover();
    return;
  }
  const provider = new CodexProvider({
    codexPath: args["codex-path"],
    allowUnverifiedVersion: args["allow-unverified-version"],
    transport: transportPref,
    includeRaw: args.raw || Boolean(args["save-fixture"]),
    log,
  });
  if (args.watch) {
    await runWatch(provider);
    return;
  }
  const code = await runOnce(provider);
  await provider.stop();
  process.exit(code);
}

main().catch((e) => {
  err(`[quotadock] fatal: ${sanitizeString(e instanceof Error ? e.stack ?? e.message : String(e))}`);
  process.exit(1);
});
