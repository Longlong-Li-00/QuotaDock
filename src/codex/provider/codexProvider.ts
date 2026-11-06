// CodexProvider: discovery -> version gate -> transport (daemon proxy, else private stdio
// app-server) -> read-only RPC -> CodexQuotaSnapshot.
//
// QuotaDock never touches credentials. Authentication, token refresh and the backend request are
// all performed by the official Codex app-server; QuotaDock only sends allowlisted read methods.

import { discoverCodexInstallations, getDaemonStatus, type DiscoveredInstallation, type DiscoveryResult } from "../discovery.ts";
import { buildCodexSnapshot } from "../parser/buildSnapshot.ts";
import { RateLimitParseError } from "../parser/parseRateLimits.ts";
import { JsonRpcError, MethodNotAllowedError, RpcClosedError, RpcTimeoutError, type JsonRpcNotification } from "../rpc/jsonRpcClient.ts";
import { sanitizeString } from "../sanitize.ts";
import { DaemonProxyTransport } from "../transport/daemonProxy.ts";
import { StdioAppServerTransport } from "../transport/stdioAppServer.ts";
import { TransportError, type ChildProcessJsonRpcTransport } from "../transport/transport.ts";
import { silentLogger, type CodexReadResult, type DaemonStatus, type InitializeResult, type Logger, type TransportKind } from "../types.ts";

export type TransportPreference = "auto" | TransportKind;

export interface CodexProviderOptions {
  /** Explicit executable; otherwise discovered. */
  codexPath?: string;
  allowUnverifiedVersion?: boolean;
  transport?: TransportPreference;
  includeRaw?: boolean;
  requestTimeoutMs?: number;
  /** Environment for child processes (tests use an isolated CODEX_HOME). */
  env?: NodeJS.ProcessEnv;
  log?: Logger;
  // Seams for tests.
  discover?: typeof discoverCodexInstallations;
  daemonStatus?: typeof getDaemonStatus;
  createTransport?: (kind: TransportKind, codexPath: string) => ChildProcessJsonRpcTransport;
}

export interface ProviderEnvironment {
  discovery: DiscoveryResult | null;
  installation: DiscoveredInstallation | null;
  daemon: DaemonStatus | null;
  transport: TransportKind | null;
  transportReason: string | null;
  initialize: InitializeResult | null;
  /** Server version parsed from the initialize userAgent (may differ from the CLI in daemon mode). */
  appServerVersion: string | null;
  pid: number | null;
  account: { type: string; planType: string | null } | null;
  /** Whether the rate-limit accountId equals account/read's chatgptAccountId on the last read. */
  accountCheck: "match" | "mismatch" | "unknown";
}

/** Background-poll parameters: skip the reset-credit detail lookup, never opt into Luna Reserve. */
export const BACKGROUND_RATE_LIMIT_PARAMS = { excludeResetCreditDetails: true, supportsLunaReserve: false } as const;

interface AccountReadResult {
  account: { type: string; planType?: string; email?: string | null } | null;
  requiresOpenaiAuth: boolean;
  workspaceRouting?: { chatgptAccountId?: string } | null;
}

export class CodexProvider {
  #opts: CodexProviderOptions;
  #log: Logger;
  #transport: ChildProcessJsonRpcTransport | null = null;
  #notificationListeners = new Set<(n: JsonRpcNotification) => void>();
  #activeAccountId: string | null = null;
  #env: ProviderEnvironment = {
    discovery: null,
    installation: null,
    daemon: null,
    transport: null,
    transportReason: null,
    initialize: null,
    appServerVersion: null,
    pid: null,
    account: null,
    accountCheck: "unknown",
  };

  constructor(options: CodexProviderOptions = {}) {
    this.#opts = options;
    this.#log = options.log ?? silentLogger;
  }

  get environment(): Readonly<ProviderEnvironment> {
    return this.#env;
  }

  onNotification(listener: (n: JsonRpcNotification) => void): () => void {
    this.#notificationListeners.add(listener);
    return () => this.#notificationListeners.delete(listener);
  }

  /** Runs discovery (once, or again after a failed discovery). Returns a failure result or null. */
  async prepare(): Promise<CodexReadResult | null> {
    if (this.#env.installation) return null;
    const discover = this.#opts.discover ?? discoverCodexInstallations;
    const discovery = await discover({ manualPath: this.#opts.codexPath, allowUnverifiedVersion: this.#opts.allowUnverifiedVersion });
    this.#env.discovery = discovery;
    if (discovery.selected) {
      this.#env.installation = discovery.selected;
      if (discovery.selected.gate !== "verified") {
        this.#log.warn(`codex ${discovery.selected.version} is ${discovery.selected.gate}; verified version is the reference`);
      }
      return null;
    }
    const older = discovery.candidates.find((c) => c.gate === "unsupported-older" || c.gate === "unparseable");
    if (older) {
      return {
        status: "unsupported-version",
        message: `only unverified codex versions found (${discovery.candidates.map((c) => c.version).join(", ")}); ${discovery.selectionReason}`,
        installation: older,
        gate: older.gate,
      };
    }
    return { status: "no-installation", message: discovery.selectionReason };
  }

  /** One read. Starts (or restarts) the persistent transport only when it is not alive. */
  async read(): Promise<CodexReadResult> {
    const failure = await this.prepare();
    if (failure) return failure;
    const installation = this.#env.installation!;

    try {
      if (!this.#transport || !this.#transport.isAlive()) {
        const authProblem = await this.#connect(installation);
        if (authProblem) {
          // Start fresh next time so a sign-in done meanwhile in the official app is picked up.
          await this.stop();
          return authProblem;
        }
      }
      const transport = this.#transport!;
      const capturedAt = new Date().toISOString();
      const raw = await transport.request<unknown>("account/rateLimits/read", BACKGROUND_RATE_LIMIT_PARAMS);
      const snapshot = buildCodexSnapshot(raw, {
        capturedAt,
        codexCliVersion: installation.version,
        transport: transport.kind,
        includeRaw: this.#opts.includeRaw,
      });
      this.#env.accountCheck =
        this.#activeAccountId && snapshot.accountId ? (this.#activeAccountId === snapshot.accountId ? "match" : "mismatch") : "unknown";
      if (this.#env.accountCheck === "mismatch") {
        snapshot.warnings.push("rate-limit accountId differs from the active Codex account (account/read)");
      }
      if (snapshot.ordinaryUsageAllowed === null) {
        snapshot.warnings.push("ordinaryUsageAllowed is null: availability unknown (backend omits it when the account/user check fails)");
      }
      return { status: "ok", snapshot };
    } catch (e) {
      const failure = this.#toFailure(e);
      if (failure.status === "not-authenticated" || failure.status === "unsupported-auth") await this.stop();
      return failure;
    }
  }

  async stop(): Promise<void> {
    await this.#transport?.stop();
    this.#transport = null;
  }

  async #connect(installation: DiscoveredInstallation): Promise<CodexReadResult | null> {
    await this.#transport?.stop();
    this.#transport = null;
    const preference = this.#opts.transport ?? "auto";
    let kind: TransportKind = "stdio-app-server";
    let reason = "forced by option";

    if (preference === "auto") {
      const daemonStatus = this.#opts.daemonStatus ?? getDaemonStatus;
      const daemon = await daemonStatus(installation.path, this.#opts.env);
      this.#env.daemon = daemon;
      if (daemon.running) {
        kind = "daemon-proxy";
        reason = "managed app-server daemon is running";
      } else {
        reason = `no managed daemon (${daemon.available ? daemon.error ?? "not running" : "daemon command unavailable"}); using a private persistent stdio app-server`;
      }
    } else {
      kind = preference;
    }

    let transport = this.#createTransport(kind, installation.path);
    try {
      await transport.start();
    } catch (e) {
      if (preference !== "auto" || kind !== "daemon-proxy") throw e;
      this.#log.warn(`daemon proxy failed (${errorMessage(e)}); falling back to stdio app-server`);
      reason = `daemon proxy failed to start (${errorMessage(e)}); fell back to private stdio app-server`;
      kind = "stdio-app-server";
      transport = this.#createTransport(kind, installation.path);
      await transport.start();
    }
    this.#transport = transport;
    transport.onNotification((n) => {
      for (const l of this.#notificationListeners) l(n);
    });
    transport.onExit((info) => this.#log.warn(`${transport.kind} child exited (code ${info.code}, signal ${info.signal}); will reconnect on next read`));

    const init = transport.initializeResult;
    this.#env = {
      ...this.#env,
      transport: kind,
      transportReason: reason,
      initialize: init,
      appServerVersion: init ? parseServerVersion(init.userAgent) : null,
      pid: transport.pid,
    };
    if (this.#env.appServerVersion && this.#env.appServerVersion !== installation.version) {
      this.#log.warn(`app-server reports ${this.#env.appServerVersion} while the CLI is ${installation.version}`);
    }

    // Read-only auth check. `refreshToken` is never set: QuotaDock does not trigger token refreshes.
    const account = await transport.request<AccountReadResult>("account/read", {});
    this.#activeAccountId = account.workspaceRouting?.chatgptAccountId ?? null;
    this.#env.account = account.account ? { type: account.account.type, planType: account.account.planType ?? null } : null;
    if (!account.account) {
      return { status: "not-authenticated", message: "Codex is not signed in. Sign in with the official Codex app or CLI; QuotaDock never logs in itself." };
    }
    if (account.account.type !== "chatgpt") {
      return {
        status: "unsupported-auth",
        authType: account.account.type,
        message: `Codex is signed in with ${account.account.type}; ChatGPT plan rate limits only exist for ChatGPT sign-in.`,
      };
    }
    return null;
  }

  #createTransport(kind: TransportKind, codexPath: string): ChildProcessJsonRpcTransport {
    if (this.#opts.createTransport) return this.#opts.createTransport(kind, codexPath);
    const common = { codexPath, env: this.#opts.env, requestTimeoutMs: this.#opts.requestTimeoutMs ?? 30_000, log: this.#log };
    return kind === "daemon-proxy" ? new DaemonProxyTransport(common) : new StdioAppServerTransport(common);
  }

  #toFailure(e: unknown): CodexReadResult {
    if (e instanceof JsonRpcError) {
      // Messages from app-server/src/request_processors/account_processor.rs (rust-v0.162.0-alpha.2).
      if (/codex account authentication required/i.test(e.message)) {
        return { status: "not-authenticated", message: "Codex is not signed in. Sign in with the official Codex app or CLI." };
      }
      if (/chatgpt authentication required/i.test(e.message)) {
        return { status: "unsupported-auth", authType: "non-chatgpt", message: sanitizeString(e.message) };
      }
      return { status: "error", errorKind: "rpc", code: e.code, message: sanitizeString(e.message) };
    }
    if (e instanceof RpcTimeoutError) return { status: "error", errorKind: "timeout", message: e.message };
    if (e instanceof RateLimitParseError) return { status: "error", errorKind: "parse", message: e.message };
    if (e instanceof TransportError || e instanceof RpcClosedError) {
      return { status: "error", errorKind: "transport", message: sanitizeString(e.message) };
    }
    if (e instanceof MethodNotAllowedError) return { status: "error", errorKind: "rpc", message: e.message };
    return { status: "error", errorKind: "transport", message: sanitizeString(errorMessage(e)) };
  }
}

/** "quotadock/0.162.0-alpha.2 (Windows ...) ..." -> "0.162.0-alpha.2" */
export function parseServerVersion(userAgent: string): string | null {
  return /^[^/\s]+\/(\d+\.\d+\.\d+[^\s]*)/.exec(userAgent)?.[1] ?? null;
}

function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
