// Preferred transport: attach to the shared, managed app-server daemon through
// `codex app-server proxy`, which relays stdio bytes to the daemon's control socket
// (CODEX_HOME/app-server-control/app-server-control.sock). Auth and token refresh then stay in
// the one long-running daemon instead of a second app-server process.
//
// The control socket speaks WebSocket (HTTP upgrade to ws://localhost/rpc), so this transport
// uses the WebSocket codec on top of the proxy's stdio. Each proxy process is a separate
// connection and needs its own initialize. Without a daemon, `proxy` exits immediately with
// "failed to connect to socket" (exit code 1). Closing the proxy does not stop the daemon.

import { ChildProcessJsonRpcTransport, type ChildTransportOptions } from "./transport.ts";

export type DaemonProxyOptions = Omit<ChildTransportOptions, "kind" | "command" | "args" | "codec"> & { codexPath: string };

export class DaemonProxyTransport extends ChildProcessJsonRpcTransport {
  constructor(options: DaemonProxyOptions) {
    const { codexPath, ...rest } = options;
    super({ ...rest, kind: "daemon-proxy", command: codexPath, args: ["app-server", "proxy"], codec: "websocket" });
  }
}
