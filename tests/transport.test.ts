// Lifecycle tests against a fake app-server child (tests/helpers/fakeAppServer.mjs). No network.

import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { MethodNotAllowedError, type JsonRpcNotification } from "../src/codex/rpc/jsonRpcClient.ts";
import type { CodecKind } from "../src/codex/transport/codecs.ts";
import { ChildProcessJsonRpcTransport, TransportError } from "../src/codex/transport/transport.ts";
import { fakeAppServerPath, realFixturePath } from "./helpers/fixtures.ts";

function fakeTransport(codec: CodecKind, ...flags: string[]) {
  return new ChildProcessJsonRpcTransport({
    kind: codec === "websocket" ? "daemon-proxy" : "stdio-app-server",
    command: process.execPath,
    args: [fakeAppServerPath, codec === "websocket" ? "ws" : "jsonl", realFixturePath, ...flags],
    codec,
    startTimeoutMs: 10_000,
    requestTimeoutMs: 5_000,
  });
}

function isProcessAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

const waitFor = async (cond: () => boolean, ms = 3_000): Promise<void> => {
  const start = Date.now();
  while (!cond()) {
    if (Date.now() - start > ms) throw new Error("condition not met in time");
    await new Promise((r) => setTimeout(r, 20));
  }
};

for (const codec of ["jsonl", "websocket"] as const) {
  describe(`ChildProcessJsonRpcTransport over ${codec}`, () => {
    it("initializes, answers allowlisted reads, refuses server requests, and stops its child", async () => {
      const t = fakeTransport(codec);
      const notes: JsonRpcNotification[] = [];
      t.onNotification((n) => notes.push(n));
      await t.start();
      assert.ok(t.isAlive());
      assert.match(t.initializeResult!.userAgent, /^quotadock\//, "clientInfo.name identifies QuotaDock");

      const limits = await t.request<{ rateLimitsByLimitId: Record<string, unknown> }>("account/rateLimits/read", { excludeResetCreditDetails: true, supportsLunaReserve: false });
      assert.deepEqual(Object.keys(limits.rateLimitsByLimitId), ["codex"]);
      await waitFor(() => notes.some((n) => n.method === "test/serverRequestAnswered"));
      assert.deepEqual(notes.find((n) => n.method === "test/serverRequestAnswered")?.params, { code: -32601 });
      assert.deepEqual(notes.find((n) => n.method === "test/echoParams")?.params, { excludeResetCreditDetails: true, supportsLunaReserve: false });
      if (codec === "websocket") {
        await waitFor(() => notes.some((n) => n.method === "test/pong"));
        assert.deepEqual(notes.find((n) => n.method === "test/pong")?.params, { payload: "hi" });
      } else {
        assert.equal(t.rpcStats?.noise, 1, "non-JSON stdout line counted as noise");
      }

      await assert.rejects(t.request("account/logout", {}), MethodNotAllowedError);
      await assert.rejects(t.request("account/rateLimitResetCredit/consume", {}), MethodNotAllowedError);

      const pid = t.pid!;
      await t.stop();
      assert.equal(t.isAlive(), false);
      await waitFor(() => !isProcessAlive(pid));
    });

    it("shares one child between concurrent start() calls", async () => {
      const t = fakeTransport(codec);
      await Promise.all([t.start(), t.start(), t.start()]);
      const pid = t.pid;
      await t.start();
      assert.equal(t.pid, pid, "no second spawn while alive");
      await t.stop();
    });

    it("detects an unexpected child exit and can restart", async () => {
      const t = fakeTransport(codec, "--crash-after-init");
      const exits: Array<{ code: number | null }> = [];
      t.onExit((e) => exits.push(e));
      await t.start();
      await waitFor(() => exits.length === 1);
      assert.equal(exits[0]!.code, 3);
      assert.equal(t.isAlive(), false);
      await assert.rejects(t.request("account/read", {}), TransportError);
      await t.restart();
      assert.ok(t.isAlive() || exits.length >= 1);
      await t.stop();
    });
  });
}

describe("WebSocket handshake failure", () => {
  it("fails start() and leaves no child behind", async () => {
    const t = fakeTransport("websocket", "--fail-ws-handshake");
    await assert.rejects(t.start(), (e: unknown) => e instanceof TransportError && /upgrade refused/.test(e.message));
    assert.equal(t.isAlive(), false);
    assert.equal(t.pid, null);
  });
});

describe("spawn failure", () => {
  it("reports a missing executable as TransportError", async () => {
    const t = new ChildProcessJsonRpcTransport({ kind: "stdio-app-server", command: "Z:\\definitely\\missing\\codex.exe", args: [], codec: "jsonl", startTimeoutMs: 5_000 });
    await assert.rejects(t.start(), TransportError);
    assert.equal(t.isAlive(), false);
  });
});
