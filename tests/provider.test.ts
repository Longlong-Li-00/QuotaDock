// CodexProvider wiring with discovery/daemon/transport seams pointed at the fake app-server.

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import type { DiscoveredInstallation, DiscoveryResult } from "../src/codex/discovery.ts";
import { CodexProvider, parseServerVersion } from "../src/codex/provider/codexProvider.ts";
import { ChildProcessJsonRpcTransport } from "../src/codex/transport/transport.ts";
import type { DaemonStatus, TransportKind } from "../src/codex/types.ts";
import { fakeAppServerPath, realFixturePath } from "./helpers/fixtures.ts";

const install: DiscoveredInstallation = { path: "C:\\fake\\codex.exe", version: "0.162.0-alpha.2", source: "desktop", gate: "verified" };
const discovered = (selected: DiscoveredInstallation | null, candidates = selected ? [selected] : []): (() => Promise<DiscoveryResult>) => async () => ({
  candidates,
  rejected: [],
  selected,
  selectionReason: selected ? "test" : "no codex executable found",
});

function makeTransport(kind: TransportKind, flags: string[] = []): ChildProcessJsonRpcTransport {
  const ws = kind === "daemon-proxy";
  return new ChildProcessJsonRpcTransport({
    kind,
    command: process.execPath,
    args: [fakeAppServerPath, ws ? "ws" : "jsonl", realFixturePath, ...flags],
    codec: ws ? "websocket" : "jsonl",
    startTimeoutMs: 10_000,
  });
}

describe("CodexProvider", () => {
  it("uses the private stdio app-server when no daemon runs, and reuses it across reads", async () => {
    const spawned: TransportKind[] = [];
    const provider = new CodexProvider({
      discover: discovered(install),
      daemonStatus: async (): Promise<DaemonStatus> => ({ available: true, running: false, error: "failed to connect" }),
      createTransport: (kind) => {
        spawned.push(kind);
        return makeTransport(kind);
      },
    });
    const params: unknown[] = [];
    provider.onNotification((n) => {
      if (n.method === "test/echoParams") params.push(n.params);
    });
    const first = await provider.read();
    assert.equal(first.status, "ok");
    if (first.status !== "ok") return;
    assert.equal(first.snapshot.transport, "stdio-app-server");
    assert.equal(first.snapshot.codexCliVersion, "0.162.0-alpha.2");
    assert.equal(first.snapshot.availability, "allowed");
    assert.equal(first.snapshot.fiveHour?.windowDurationMins, 300);
    const pid = provider.environment.pid;
    const second = await provider.read();
    assert.equal(second.status, "ok");
    assert.deepEqual(spawned, ["stdio-app-server"], "one persistent child, not one per poll");
    assert.equal(provider.environment.pid, pid);
    assert.deepEqual(params[0], { excludeResetCreditDetails: true, supportsLunaReserve: false });
    assert.equal(provider.environment.account?.type, "chatgpt");
    assert.equal(provider.environment.accountCheck, "match");
    assert.equal(provider.environment.appServerVersion, "0.162.0-alpha.2");
    await provider.stop();
  });

  it("prefers the daemon proxy and falls back to stdio when the proxy fails", async () => {
    const spawned: TransportKind[] = [];
    const provider = new CodexProvider({
      discover: discovered(install),
      daemonStatus: async () => ({ available: true, running: true, version: "0.162.0-alpha.2" }),
      createTransport: (kind) => {
        spawned.push(kind);
        return makeTransport(kind, kind === "daemon-proxy" ? ["--fail-ws-handshake"] : []);
      },
    });
    const r = await provider.read();
    assert.equal(r.status, "ok");
    assert.deepEqual(spawned, ["daemon-proxy", "stdio-app-server"]);
    assert.match(provider.environment.transportReason ?? "", /fell back/);
    await provider.stop();
  });

  it("uses the daemon proxy when it works", async () => {
    const provider = new CodexProvider({
      discover: discovered(install),
      daemonStatus: async () => ({ available: true, running: true }),
      createTransport: (kind) => makeTransport(kind),
    });
    const r = await provider.read();
    assert.equal(r.status, "ok");
    if (r.status === "ok") assert.equal(r.snapshot.transport, "daemon-proxy");
    await provider.stop();
  });

  it("reports no-installation and unsupported-version without spawning anything", async () => {
    let spawned = 0;
    const none = new CodexProvider({ discover: discovered(null), createTransport: (k) => (spawned++, makeTransport(k)) });
    assert.equal((await none.read()).status, "no-installation");

    const old: DiscoveredInstallation = { ...install, version: "0.154.0-alpha.6.2", source: "vscode", gate: "unsupported-older" };
    const tooOld = new CodexProvider({ discover: discovered(null, [old]), createTransport: (k) => (spawned++, makeTransport(k)) });
    const r = await tooOld.read();
    assert.equal(r.status, "unsupported-version");
    assert.equal(spawned, 0);
  });

  it("parses the server version from the initialize userAgent", () => {
    assert.equal(parseServerVersion("quotadock/0.162.0-alpha.2 (Windows 10.0.26200; x86_64) unknown (quotadock; 0.1.0)"), "0.162.0-alpha.2");
    assert.equal(parseServerVersion("garbage"), null);
  });
});
