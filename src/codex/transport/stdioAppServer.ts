// Fallback transport: QuotaDock's own persistent app-server child over stdio (JSONL).
// `--listen stdio://` is the documented default in `codex app-server --help` (0.162.0-alpha.2).
// One QuotaDock runtime owns exactly one of these; it is reused across polls, never re-spawned per
// poll. It exits by itself when its stdin closes, so it cannot outlive QuotaDock.

import { ChildProcessJsonRpcTransport, type ChildTransportOptions } from "./transport.ts";

export type StdioAppServerOptions = Omit<ChildTransportOptions, "kind" | "command" | "args" | "codec"> & { codexPath: string };

export class StdioAppServerTransport extends ChildProcessJsonRpcTransport {
  constructor(options: StdioAppServerOptions) {
    const { codexPath, ...rest } = options;
    super({ ...rest, kind: "stdio-app-server", command: codexPath, args: ["app-server", "--listen", "stdio://"], codec: "jsonl" });
  }
}
