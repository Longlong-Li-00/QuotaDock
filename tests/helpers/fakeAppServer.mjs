// A stand-in for `codex app-server` used by transport/provider tests. Never talks to a network.
//
//   node fakeAppServer.mjs <jsonl|ws> <fixture.json> [--crash-after-init] [--fail-ws-handshake]
//
// Behaviour mirrors what was observed on codex-cli 0.162.0-alpha.2: requests before initialize
// get -32600 "Not initialized"; no "jsonrpc" member; stdio mode exits when stdin closes.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const [mode, fixturePath, ...flags] = process.argv.slice(2);
const rateLimits = JSON.parse(readFileSync(fixturePath, "utf8")).response;
const crashAfterInit = flags.includes("--crash-after-init");
const failHandshake = flags.includes("--fail-ws-handshake");
let initialized = false;

function handle(msg, send) {
  if (msg.id === "srv-1" && "error" in msg) {
    send({ method: "test/serverRequestAnswered", params: { code: msg.error.code } });
    return;
  }
  if (!("id" in msg)) return; // client notifications (e.g. "initialized") are only logged upstream
  if (msg.method !== "initialize" && !initialized) {
    send({ id: msg.id, error: { code: -32600, message: "Not initialized" } });
    return;
  }
  switch (msg.method) {
    case "initialize":
      initialized = true;
      send({ id: msg.id, result: { userAgent: `${msg.params.clientInfo.name}/0.162.0-alpha.2 (fake)`, codexHome: "C:\\fake-codex-home", platformFamily: "windows", platformOs: "windows" } });
      send({ method: "remoteControl/status/changed", params: { status: "disabled", installationId: "00000000-0000-0000-0000-000000000000" } });
      send({ id: "srv-1", method: "item/tool/call", params: {} }); // server-initiated request the client must refuse
      if (crashAfterInit) setTimeout(() => process.exit(3), 50);
      break;
    case "account/read":
      send({ method: "account/updated", params: { authMode: "chatgpt", planType: "plus" } });
      send({ id: msg.id, result: { account: { type: "chatgpt", email: "someone@example.com", planType: "plus" }, requiresOpenaiAuth: true, workspaceRouting: { chatgptAccountId: rateLimits.accountId ?? "acct_fake" } } });
      break;
    case "account/rateLimits/read":
      send({ method: "test/echoParams", params: msg.params ?? null });
      send({ id: msg.id, result: rateLimits });
      break;
    default:
      send({ id: msg.id, error: { code: -32600, message: `Invalid request: unknown variant \`${msg.method}\`` } });
  }
}

if (mode === "jsonl") {
  let buf = "";
  const send = (o) => process.stdout.write(JSON.stringify(o) + "\n");
  process.stdout.write("this line is not JSON\n"); // noise the client must tolerate
  process.stdin.setEncoding("utf8");
  process.stdin.on("data", (c) => {
    buf += c;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (line.trim()) handle(JSON.parse(line), send);
    }
  });
  process.stdin.on("end", () => process.exit(0));
} else {
  // WebSocket server side over stdio, as the daemon socket would look through `app-server proxy`.
  let buf = Buffer.alloc(0);
  let upgraded = false;
  const frame = (opcode, payload, fin = true) => {
    const len = payload.length;
    const head = len < 126 ? Buffer.from([(fin ? 0x80 : 0) | opcode, len]) : Buffer.from([(fin ? 0x80 : 0) | opcode, 126, len >> 8, len & 0xff]);
    return Buffer.concat([head, payload]);
  };
  let first = true;
  const send = (o) => {
    const data = Buffer.from(JSON.stringify(o));
    if (first && data.length > 8) {
      // Exercise fragmentation and an interleaved ping on the first message.
      first = false;
      process.stdout.write(Buffer.concat([frame(0x1, data.subarray(0, 5), false), frame(0x9, Buffer.from("hi")), frame(0x0, data.subarray(5), true)]));
      return;
    }
    process.stdout.write(frame(0x1, data));
  };
  process.stdin.on("data", (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    if (!upgraded) {
      const end = buf.indexOf("\r\n\r\n");
      if (end < 0) return;
      const head = buf.subarray(0, end).toString("latin1");
      buf = buf.subarray(end + 4);
      const key = /sec-websocket-key:\s*(\S+)/i.exec(head)?.[1] ?? "";
      if (failHandshake || !/^GET \/rpc HTTP\/1\.1/.test(head)) {
        process.stdout.write("HTTP/1.1 400 Bad Request\r\n\r\n");
        return;
      }
      const accept = createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
      process.stdout.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
      upgraded = true;
    }
    for (;;) {
      if (buf.length < 2) return;
      const opcode = buf[0] & 0x0f;
      const masked = (buf[1] & 0x80) !== 0;
      let len = buf[1] & 0x7f;
      let off = 2;
      if (len === 126) {
        if (buf.length < 4) return;
        len = buf.readUInt16BE(2);
        off = 4;
      } else if (len === 127) {
        if (buf.length < 10) return;
        len = Number(buf.readBigUInt64BE(2));
        off = 10;
      }
      if (!masked) process.exit(9); // RFC 6455: client frames must be masked
      if (buf.length < off + 4 + len) return;
      const mask = buf.subarray(off, off + 4);
      const payload = Buffer.from(buf.subarray(off + 4, off + 4 + len).map((b, i) => b ^ mask[i % 4]));
      buf = buf.subarray(off + 4 + len);
      if (opcode === 0x1) handle(JSON.parse(payload.toString("utf8")), send);
      else if (opcode === 0xa) send({ method: "test/pong", params: { payload: payload.toString() } });
      else if (opcode === 0x8) process.stdout.write(frame(0x8, payload));
    }
  });
  process.stdin.on("end", () => process.exit(0));
}
