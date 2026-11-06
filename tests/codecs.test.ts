import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { encodeClientFrame, JsonlCodec, WebSocketCodec, websocketAccept } from "../src/codex/transport/codecs.ts";

/** Decodes a (masked) client frame written by the codec. */
function decodeClientFrame(frame: Buffer): { fin: boolean; opcode: number; masked: boolean; payload: Buffer } {
  let len = frame[1]! & 0x7f;
  let off = 2;
  if (len === 126) {
    len = frame.readUInt16BE(2);
    off = 4;
  } else if (len === 127) {
    len = Number(frame.readBigUInt64BE(2));
    off = 10;
  }
  const masked = (frame[1]! & 0x80) !== 0;
  const mask = frame.subarray(off, off + 4);
  const body = frame.subarray(off + 4, off + 4 + len);
  return { fin: (frame[0]! & 0x80) !== 0, opcode: frame[0]! & 0x0f, masked, payload: Buffer.from(body.map((b, i) => b ^ mask[i % 4]!)) };
}

function openedCodec() {
  const written: Buffer[] = [];
  const messages: string[] = [];
  const noise: string[] = [];
  const closes: string[] = [];
  const codec = new WebSocketCodec({ write: (d) => written.push(d), onMessage: (m) => messages.push(m), onNoise: (n) => noise.push(n), onPeerClose: (r) => closes.push(r) });
  const opened = codec.open();
  const request = written[0]!.toString("latin1");
  const key = /Sec-WebSocket-Key: (\S+)/.exec(request)![1]!;
  const accept = (k = key) => Buffer.from(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${websocketAccept(k)}\r\n\r\n`, "latin1");
  return { codec, opened, request, accept, written, messages, noise, closes };
}

describe("WebSocketCodec (RFC 6455 test vectors)", () => {
  it("computes Sec-WebSocket-Accept as in RFC 6455 section 1.3", () => {
    assert.equal(websocketAccept("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
  });

  it("sends the upgrade request the daemon socket expects", () => {
    const { request } = openedCodec();
    assert.match(request, /^GET \/rpc HTTP\/1\.1\r\n/);
    assert.match(request, /\r\nHost: localhost\r\n/);
    assert.match(request, /\r\nUpgrade: websocket\r\n/);
    assert.match(request, /\r\nSec-WebSocket-Version: 13\r\n\r\n$/);
  });

  it("decodes an unmasked single-frame text message arriving with the handshake (RFC 5.7)", async () => {
    const { codec, opened, accept, messages } = openedCodec();
    codec.push(Buffer.concat([accept(), Buffer.from([0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f])]));
    await opened;
    assert.deepEqual(messages, ["Hello"]);
  });

  it("decodes a masked frame and a fragmented message split at arbitrary byte boundaries (RFC 5.7)", async () => {
    const { codec, opened, accept, messages } = openedCodec();
    codec.push(accept());
    await opened;
    codec.push(Buffer.from([0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58]));
    const fragmented = Buffer.from([0x01, 0x03, 0x48, 0x65, 0x6c, 0x80, 0x02, 0x6c, 0x6f]);
    for (const byte of fragmented) codec.push(Buffer.from([byte]));
    assert.deepEqual(messages, ["Hello", "Hello"]);
  });

  it("answers ping with a masked pong carrying the same payload", async () => {
    const { codec, opened, accept, written } = openedCodec();
    codec.push(accept());
    await opened;
    codec.push(Buffer.from([0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]));
    const pong = decodeClientFrame(written.at(-1)!);
    assert.equal(pong.opcode, 0xa);
    assert.equal(pong.masked, true);
    assert.equal(pong.payload.toString(), "Hello");
  });

  it("masks outgoing text frames for every length encoding", async () => {
    const { codec, opened, accept, written } = openedCodec();
    codec.push(accept());
    await opened;
    for (const size of [5, 200, 70_000]) {
      const text = "x".repeat(size);
      codec.send(text);
      const f = decodeClientFrame(written.at(-1)!);
      assert.equal(f.opcode, 0x1);
      assert.equal(f.fin, true);
      assert.equal(f.masked, true);
      assert.equal(f.payload.toString(), text);
    }
  });

  it("reports a close frame from the peer and replies with close", async () => {
    const { codec, opened, accept, written, closes } = openedCodec();
    codec.push(accept());
    await opened;
    codec.push(Buffer.from([0x88, 0x02, 0x03, 0xe8]));
    assert.equal(closes.length, 1);
    assert.match(closes[0]!, /1000/);
    assert.equal(decodeClientFrame(written.at(-1)!).opcode, 0x8);
  });

  it("rejects a refused upgrade", async () => {
    const { codec, opened } = openedCodec();
    codec.push(Buffer.from("HTTP/1.1 400 Bad Request\r\n\r\n"));
    await assert.rejects(opened, /upgrade refused/);
  });

  it("rejects a wrong Sec-WebSocket-Accept", async () => {
    const { codec, opened, accept } = openedCodec();
    codec.push(accept("AAAAAAAAAAAAAAAAAAAAAA=="));
    await assert.rejects(opened, /Sec-WebSocket-Accept/);
  });

  it("refuses to send before the handshake", () => {
    const codec = new WebSocketCodec({ write: () => {}, onMessage: () => {}, onNoise: () => {} });
    assert.throws(() => codec.send("{}"), /handshake not complete/);
  });

  it("encodeClientFrame sets FIN, opcode and mask bit", () => {
    const f = encodeClientFrame(0x1, Buffer.from("hi"));
    assert.equal(f[0], 0x81);
    assert.equal(f[1]! & 0x80, 0x80);
  });
});

describe("JsonlCodec", () => {
  it("splits lines across chunks, strips CR and survives split UTF-8 sequences", () => {
    const messages: string[] = [];
    const codec = new JsonlCodec({ write: () => {}, onMessage: (m) => messages.push(m), onNoise: () => {} });
    const bytes = Buffer.from('{"a":"日本"}\r\n\n{"b":2}\n', "utf8");
    for (let i = 0; i < bytes.length; i += 3) codec.push(bytes.subarray(i, i + 3));
    assert.deepEqual(messages, ['{"a":"日本"}', '{"b":2}']);
  });

  it("appends exactly one newline per message", () => {
    const written: Buffer[] = [];
    new JsonlCodec({ write: (d) => written.push(d), onMessage: () => {}, onNoise: () => {} }).send('{"id":1}');
    assert.equal(written[0]!.toString(), '{"id":1}\n');
  });

  it("drops an unterminated runaway line as noise", () => {
    const noise: string[] = [];
    const codec = new JsonlCodec({ write: () => {}, onMessage: () => {}, onNoise: (n) => noise.push(n) }, 16);
    codec.push(Buffer.from("x".repeat(40)));
    assert.equal(noise.length, 1);
  });
});
