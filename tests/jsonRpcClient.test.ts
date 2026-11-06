import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { JsonRpcClient, JsonRpcError, MethodNotAllowedError, RpcClosedError, RpcTimeoutError, type JsonRpcNotification } from "../src/codex/rpc/jsonRpcClient.ts";
import { CLIENT_NOTIFICATION_METHODS, READ_ONLY_REQUEST_METHODS } from "../src/codex/types.ts";

function setup(timeoutMs = 1_000) {
  const sent: Array<Record<string, unknown>> = [];
  const notifications: JsonRpcNotification[] = [];
  const noise: string[] = [];
  const serverRequests: string[] = [];
  const client = new JsonRpcClient({
    write: (m) => sent.push(JSON.parse(m)),
    allowedRequestMethods: READ_ONLY_REQUEST_METHODS,
    allowedNotificationMethods: CLIENT_NOTIFICATION_METHODS,
    defaultTimeoutMs: timeoutMs,
    onNotification: (n) => notifications.push(n),
    onNoise: (t) => noise.push(t),
    onServerRequest: (m) => serverRequests.push(m),
  });
  return { client, sent, notifications, noise, serverRequests };
}

describe("JsonRpcClient", () => {
  it("sends id/method/params without a jsonrpc member and resolves the matching response", async () => {
    const { client, sent } = setup();
    const p = client.request("account/rateLimits/read", { excludeResetCreditDetails: true });
    assert.deepEqual(sent[0], { id: 1, method: "account/rateLimits/read", params: { excludeResetCreditDetails: true } });
    client.handleMessage(JSON.stringify({ id: 1, result: { ok: true } }));
    assert.deepEqual(await p, { ok: true });
  });

  it("correlates out-of-order responses by id", async () => {
    const { client } = setup();
    const a = client.request("account/read", {});
    const b = client.request("account/rateLimits/read");
    client.handleMessage(JSON.stringify({ id: 2, result: "second" }));
    client.handleMessage(JSON.stringify({ id: 1, result: "first" }));
    assert.equal(await a, "first");
    assert.equal(await b, "second");
  });

  it("omits params when undefined", () => {
    const { client, sent } = setup();
    void client.request("account/rateLimits/read").catch(() => {});
    assert.equal("params" in sent[0]!, false);
    client.close("test over");
  });

  it("turns error responses into JsonRpcError with the server code", async () => {
    const { client } = setup();
    const p = client.request("account/rateLimits/read");
    client.handleMessage(JSON.stringify({ id: 1, error: { code: -32600, message: "codex account authentication required to read rate limits" } }));
    await assert.rejects(p, (e: unknown) => e instanceof JsonRpcError && e.code === -32600 && /authentication required/.test(e.message));
  });

  it("separates notifications, noise and server requests", () => {
    const { client, sent, notifications, noise, serverRequests } = setup();
    client.handleMessage(JSON.stringify({ method: "account/rateLimits/updated", params: { x: 1 } }));
    client.handleMessage("not json at all");
    client.handleMessage("[1,2,3]");
    client.handleMessage(JSON.stringify({ foo: "bar" }));
    client.handleMessage(JSON.stringify({ id: "srv-9", method: "item/tool/call", params: {} }));
    assert.deepEqual(notifications, [{ method: "account/rateLimits/updated", params: { x: 1 } }]);
    assert.equal(noise.length, 3);
    assert.deepEqual(serverRequests, ["item/tool/call"]);
    // The server request is refused, never executed.
    assert.deepEqual(sent.at(-1), { id: "srv-9", error: { code: -32601, message: "QuotaDock is a read-only client and serves no requests" } });
    assert.equal(client.stats.notifications, 1);
    assert.equal(client.stats.serverRequests, 1);
    assert.equal(client.stats.noise, 3);
  });

  it("refuses methods outside the read-only allowlist before writing anything", async () => {
    const { client, sent } = setup();
    for (const method of ["account/logout", "account/login/start", "account/rateLimitResetCredit/consume", "account/sendAddCreditsNudgeEmail", "fs/writeFile", "process/spawn", "config/value/write"]) {
      await assert.rejects(client.request(method, {}), MethodNotAllowedError);
    }
    assert.throws(() => client.notify("thread/start"), MethodNotAllowedError);
    assert.equal(sent.length, 0);
  });

  it("times out a request that never gets a response", async () => {
    const { client } = setup(30);
    await assert.rejects(client.request("account/read", {}), RpcTimeoutError);
    assert.equal(client.pendingCount, 0);
  });

  it("close() rejects pending requests and blocks new ones", async () => {
    const { client } = setup();
    const p = client.request("account/read", {});
    client.close("child exited");
    await assert.rejects(p, RpcClosedError);
    await assert.rejects(client.request("account/read", {}), RpcClosedError);
  });

  it("counts responses that match no pending request", () => {
    const { client } = setup();
    client.handleMessage(JSON.stringify({ id: 99, result: {} }));
    assert.equal(client.stats.unmatchedResponses, 1);
  });
});
