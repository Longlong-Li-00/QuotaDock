// A persistent child process that carries app-server JSON-RPC over its stdin/stdout.
// Both transports (daemon proxy and private stdio app-server) are this class with different
// arguments and framing codecs.

import { spawn, type ChildProcess } from "node:child_process";
import { JsonRpcClient, RpcClosedError, type JsonRpcNotification } from "../rpc/jsonRpcClient.ts";
import { sanitizeString } from "../sanitize.ts";
import {
  CLIENT_NOTIFICATION_METHODS,
  READ_ONLY_REQUEST_METHODS,
  silentLogger,
  type InitializeResult,
  type Logger,
  type TransportKind,
} from "../types.ts";
import { createCodec, type CodecKind, type MessageCodec } from "./codecs.ts";

export interface CodexTransportManager {
  readonly kind: TransportKind;
  start(): Promise<void>;
  request<T>(method: string, params?: unknown): Promise<T>;
  isAlive(): boolean;
  restart(): Promise<void>;
  stop(): Promise<void>;
}

export interface ClientInfo {
  name: string;
  title?: string;
  version: string;
}

// The name becomes part of the app-server's User-Agent, so backend requests made on QuotaDock's
// behalf identify it. Never reuse "Codex Desktop": the app-server special-cases that name.
export const QUOTADOCK_CLIENT_INFO: ClientInfo = { name: "quotadock", title: "QuotaDock", version: "0.1.0" };

export interface ChildTransportOptions {
  kind: TransportKind;
  command: string;
  args: string[];
  codec: CodecKind;
  clientInfo?: ClientInfo;
  env?: NodeJS.ProcessEnv;
  requestTimeoutMs?: number;
  /** Covers spawn + framing handshake + initialize. */
  startTimeoutMs?: number;
  log?: Logger;
}

export class TransportError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TransportError";
  }
}

export interface ExitInfo {
  code: number | null;
  signal: string | null;
}

export class ChildProcessJsonRpcTransport implements CodexTransportManager {
  readonly kind: TransportKind;
  #opts: ChildTransportOptions;
  #log: Logger;
  #child: ChildProcess | null = null;
  #codec: MessageCodec | null = null;
  #rpc: JsonRpcClient | null = null;
  #starting: Promise<void> | null = null;
  #initialized = false;
  #initializeResult: InitializeResult | null = null;
  #stderrTail: string[] = [];
  /** Children we are deliberately stopping; their exit is not reported as unexpected. */
  #stopping = new WeakSet<ChildProcess>();
  #listeners = new Set<(n: JsonRpcNotification) => void>();
  #exitListeners = new Set<(info: ExitInfo) => void>();
  #killOnExit = (): void => {
    this.#child?.kill();
  };

  constructor(options: ChildTransportOptions) {
    this.kind = options.kind;
    this.#opts = options;
    this.#log = options.log ?? silentLogger;
  }

  get initializeResult(): InitializeResult | null {
    return this.#initializeResult;
  }

  get pid(): number | null {
    return this.#child?.pid ?? null;
  }

  /** Last stderr lines of the child, sanitized. */
  get stderrTail(): readonly string[] {
    return this.#stderrTail;
  }

  get rpcStats(): JsonRpcClient["stats"] | null {
    return this.#rpc?.stats ?? null;
  }

  onNotification(listener: (n: JsonRpcNotification) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  onExit(listener: (info: ExitInfo) => void): () => void {
    this.#exitListeners.add(listener);
    return () => this.#exitListeners.delete(listener);
  }

  isAlive(): boolean {
    const c = this.#child;
    return Boolean(c && c.exitCode === null && c.signalCode === null && this.#initialized && this.#rpc && !this.#rpc.closed);
  }

  /** Spawns the child once and completes the handshake. Concurrent calls share one start. */
  start(): Promise<void> {
    if (this.isAlive()) return Promise.resolve();
    if (!this.#starting) {
      this.#starting = this.#doStart().finally(() => {
        this.#starting = null;
      });
    }
    return this.#starting;
  }

  async request<T>(method: string, params?: unknown): Promise<T> {
    if (!this.isAlive() || !this.#rpc) throw new TransportError(`${this.kind} transport is not running`);
    return this.#rpc.request<T>(method, params, this.#opts.requestTimeoutMs);
  }

  async restart(): Promise<void> {
    await this.stop();
    await this.start();
  }

  /** Closes the channel politely and ends stdin; escalates to kill if the child lingers. */
  async stop(): Promise<void> {
    const child = this.#child;
    this.#rpc?.close("transport stopped");
    this.#initialized = false;
    if (!child) return;
    this.#stopping.add(child);
    if (child.exitCode === null && child.signalCode === null) {
      const exited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
      this.#codec?.close();
      child.stdin?.end();
      if (!(await settlesWithin(exited, 3_000))) {
        this.#log.warn(`${this.kind}: child ${child.pid} did not exit after stdin EOF; killing`);
        child.kill();
        await settlesWithin(exited, 2_000);
      }
    }
    process.removeListener("exit", this.#killOnExit);
    if (this.#child === child) {
      this.#child = null;
      this.#codec = null;
    }
  }

  async #doStart(): Promise<void> {
    await this.stop();
    this.#stderrTail = [];
    const { command, args } = this.#opts;
    this.#log.debug(`${this.kind}: spawning ${command} ${args.join(" ")}`);
    const child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, env: this.#opts.env ?? process.env });
    this.#child = child;

    const rpc = new JsonRpcClient({
      write: (message) => codec.send(message),
      allowedRequestMethods: READ_ONLY_REQUEST_METHODS,
      allowedNotificationMethods: CLIENT_NOTIFICATION_METHODS,
      defaultTimeoutMs: this.#opts.requestTimeoutMs ?? 30_000,
      onNotification: (n) => {
        for (const l of this.#listeners) l(n);
      },
      onNoise: (text) => this.#log.debug(`${this.kind}: non-protocol output: ${sanitizeString(text).slice(0, 200)}`),
      onServerRequest: (method) => this.#log.warn(`${this.kind}: refused server request ${method}`),
    });
    const codec = createCodec(this.#opts.codec, {
      write: (data) => {
        if (!child.stdin || child.stdin.destroyed || !child.stdin.writable) throw new RpcClosedError("stdin unavailable");
        child.stdin.write(data);
      },
      onMessage: (text) => rpc.handleMessage(text),
      onNoise: (text) => this.#log.debug(`${this.kind}: ${sanitizeString(text).slice(0, 200)}`),
      onPeerClose: (reason) => {
        this.#log.warn(`${this.kind}: peer closed channel: ${reason}`);
        rpc.close(`peer closed channel: ${reason}`);
        this.#initialized = false;
      },
    });
    this.#rpc = rpc;
    this.#codec = codec;

    child.stdout?.on("data", (chunk: Buffer) => codec.push(chunk));
    child.stderr?.setEncoding("utf8");
    child.stderr?.on("data", (chunk: string) => {
      for (const line of chunk.split(/\r?\n/).filter((l) => l.trim())) {
        this.#stderrTail.push(sanitizeString(line).slice(0, 500));
        if (this.#stderrTail.length > 50) this.#stderrTail.shift();
      }
    });
    child.stdin?.on("error", () => {}); // EPIPE after the child died is reported via "exit"

    const spawned = new Promise<void>((resolve, reject) => {
      child.once("spawn", () => resolve());
      child.once("error", (e) => reject(new TransportError(`failed to spawn ${command}: ${e.message}`)));
    });
    const exitedEarly = new Promise<never>((_, reject) => {
      child.once("exit", (code, signal) => reject(new TransportError(`child exited during startup (code ${code}, signal ${signal})`)));
    });
    exitedEarly.catch(() => {}); // only observed through the race below
    child.on("error", (e) => this.#log.warn(`${this.kind}: child process error: ${sanitizeString(e.message)}`));
    child.on("exit", (code, signal) => {
      rpc.close(`child exited (code ${code}, signal ${signal})`);
      if (this.#child !== child) return; // a stale child from before a restart
      this.#initialized = false;
      process.removeListener("exit", this.#killOnExit);
      if (this.#stopping.has(child)) return;
      for (const l of this.#exitListeners) l({ code, signal });
    });
    process.once("exit", this.#killOnExit);

    try {
      await raceStartup(
        (async () => {
          await spawned;
          await codec.open();
          // Requests before `initialize` are rejected with -32600 "Not initialized".
          const result = await rpc.request<InitializeResult>(
            "initialize",
            { clientInfo: this.#opts.clientInfo ?? QUOTADOCK_CLIENT_INFO, capabilities: { experimentalApi: false } },
            this.#opts.startTimeoutMs ?? 20_000,
          );
          // Not required by the server (client notifications are only logged), sent for protocol hygiene.
          rpc.notify("initialized");
          return result;
        })().then((result) => {
          this.#initializeResult = result;
          this.#initialized = true;
        }),
        exitedEarly,
        this.#opts.startTimeoutMs ?? 20_000,
      );
      this.#log.debug(`${this.kind}: initialized (pid ${child.pid})`);
    } catch (e) {
      const tail = this.#stderrTail.slice(-3).join(" | ");
      await this.stop();
      const message = e instanceof Error ? e.message : String(e);
      throw new TransportError(`${this.kind} failed to start: ${message}${tail ? ` (stderr: ${tail})` : ""}`);
    }
  }
}

async function raceStartup(work: Promise<void>, exitedEarly: Promise<never>, timeoutMs: number): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new TransportError(`startup timed out after ${timeoutMs} ms`)), timeoutMs);
  });
  try {
    await Promise.race([work, exitedEarly, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

async function settlesWithin(p: Promise<void>, ms: number): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<false>((resolve) => {
    timer = setTimeout(() => resolve(false), ms);
  });
  const done = await Promise.race([p.then(() => true as const), timeout]);
  clearTimeout(timer);
  return done;
}
