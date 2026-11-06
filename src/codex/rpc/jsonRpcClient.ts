// Transport-agnostic JSON-RPC client for the Codex app-server message format.
//
// Framing (JSONL lines on stdio, WebSocket text frames on the daemon socket) is handled by a
// codec in ../transport/codecs.ts; this client only sees whole messages. Observed on
// codex-cli 0.162.0-alpha.2: no "jsonrpc": "2.0" member, numeric or string ids. Inbound
// messages are separated into four kinds:
//   response        { id, result } | { id, error }
//   server request  { id, method, params }   -> answered with an error; QuotaDock serves nothing
//   notification    { method, params }
//   noise           anything that is not a JSON object (logged, never parsed further)

export type RequestId = number | string;

export interface JsonRpcErrorObject {
  code: number;
  message: string;
  data?: unknown;
}

export class JsonRpcError extends Error {
  code: number;
  data: unknown;
  method: string;
  constructor(method: string, error: JsonRpcErrorObject) {
    super(`${method} failed (${error.code}): ${error.message}`);
    this.name = "JsonRpcError";
    this.code = error.code;
    this.data = error.data;
    this.method = method;
  }
}

export class RpcTimeoutError extends Error {
  method: string;
  constructor(method: string, timeoutMs: number) {
    super(`${method} timed out after ${timeoutMs} ms`);
    this.name = "RpcTimeoutError";
    this.method = method;
  }
}

export class RpcClosedError extends Error {
  constructor(reason: string) {
    super(`JSON-RPC connection closed: ${reason}`);
    this.name = "RpcClosedError";
  }
}

export class MethodNotAllowedError extends Error {
  constructor(method: string) {
    super(`method "${method}" is not on QuotaDock's read-only allowlist`);
    this.name = "MethodNotAllowedError";
  }
}

export interface JsonRpcNotification {
  method: string;
  params?: unknown;
}

export interface JsonRpcClientOptions {
  /** Sends one serialized message; the codec adds framing. */
  write: (message: string) => void;
  allowedRequestMethods: ReadonlySet<string>;
  allowedNotificationMethods: ReadonlySet<string>;
  defaultTimeoutMs?: number;
  onNotification?: (n: JsonRpcNotification) => void;
  onNoise?: (text: string) => void;
  onServerRequest?: (method: string) => void;
}

interface Pending {
  method: string;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

export class JsonRpcClient {
  #opts: JsonRpcClientOptions;
  #pending = new Map<RequestId, Pending>();
  #nextId = 1;
  #closed: string | null = null;

  readonly stats = { responses: 0, notifications: 0, serverRequests: 0, noise: 0, unmatchedResponses: 0 };

  constructor(options: JsonRpcClientOptions) {
    this.#opts = options;
  }

  get closed(): boolean {
    return this.#closed !== null;
  }

  /** Dispatches one complete inbound message. */
  handleMessage(text: string): void {
    if (!text.trim()) return;
    let msg: unknown;
    try {
      msg = JSON.parse(text);
    } catch {
      this.#noise(text);
      return;
    }
    if (!msg || typeof msg !== "object" || Array.isArray(msg)) {
      this.#noise(text);
      return;
    }
    const m = msg as Record<string, unknown>;
    const hasId = typeof m.id === "number" || typeof m.id === "string";
    const hasMethod = typeof m.method === "string";

    if (hasId && !hasMethod && ("result" in m || "error" in m)) {
      this.#handleResponse(m.id as RequestId, m);
    } else if (hasId && hasMethod) {
      this.stats.serverRequests++;
      this.#opts.onServerRequest?.(m.method as string);
      // QuotaDock never accepts work from the server (approvals, tool calls, token refresh...).
      try {
        this.#write({ id: m.id, error: { code: -32601, message: "QuotaDock is a read-only client and serves no requests" } });
      } catch {
        // channel already closing; nothing to refuse to
      }
    } else if (hasMethod) {
      this.stats.notifications++;
      this.#opts.onNotification?.({ method: m.method as string, params: m.params });
    } else {
      this.#noise(text);
    }
  }

  request<T = unknown>(method: string, params?: unknown, timeoutMs?: number): Promise<T> {
    if (!this.#opts.allowedRequestMethods.has(method)) return Promise.reject(new MethodNotAllowedError(method));
    if (this.#closed !== null) return Promise.reject(new RpcClosedError(this.#closed));
    const id = this.#nextId++;
    const timeout = timeoutMs ?? this.#opts.defaultTimeoutMs ?? 30_000;
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new RpcTimeoutError(method, timeout));
      }, timeout);
      this.#pending.set(id, { method, resolve: resolve as (v: unknown) => void, reject, timer });
      const payload: Record<string, unknown> = { id, method };
      if (params !== undefined) payload.params = params;
      try {
        this.#write(payload);
      } catch (e) {
        clearTimeout(timer);
        this.#pending.delete(id);
        reject(e instanceof Error ? e : new Error(String(e)));
      }
    });
  }

  notify(method: string, params?: unknown): void {
    if (!this.#opts.allowedNotificationMethods.has(method)) throw new MethodNotAllowedError(method);
    if (this.#closed !== null) throw new RpcClosedError(this.#closed);
    const payload: Record<string, unknown> = { method };
    if (params !== undefined) payload.params = params;
    this.#write(payload);
  }

  /** Rejects every pending request. Idempotent. */
  close(reason: string): void {
    if (this.#closed !== null) return;
    this.#closed = reason;
    for (const [id, p] of this.#pending) {
      clearTimeout(p.timer);
      p.reject(new RpcClosedError(reason));
      this.#pending.delete(id);
    }
  }

  get pendingCount(): number {
    return this.#pending.size;
  }

  #handleResponse(id: RequestId, m: Record<string, unknown>): void {
    const pending = this.#pending.get(id);
    if (!pending) {
      this.stats.unmatchedResponses++;
      return;
    }
    this.stats.responses++;
    this.#pending.delete(id);
    clearTimeout(pending.timer);
    if ("error" in m && m.error && typeof m.error === "object") {
      const e = m.error as Partial<JsonRpcErrorObject>;
      pending.reject(new JsonRpcError(pending.method, { code: typeof e.code === "number" ? e.code : -32603, message: String(e.message ?? "unknown error"), data: e.data }));
    } else {
      pending.resolve(m.result);
    }
  }

  #noise(line: string): void {
    this.stats.noise++;
    this.#opts.onNoise?.(line);
  }

  #write(payload: unknown): void {
    this.#opts.write(JSON.stringify(payload));
  }
}
