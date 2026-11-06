// Message framing between QuotaDock and an app-server child process.
//
//  - JsonlCodec: `codex app-server --listen stdio://` reads and writes one JSON message per line
//    [OPENAI CODEX SOURCE app-server-transport/src/transport/stdio.rs @ rust-v0.162.0-alpha.2].
//  - WebSocketCodec: the managed daemon's control socket accepts a WebSocket upgrade
//    (`ws://localhost/rpc`) [OPENAI CODEX SOURCE app-server-transport/src/transport/unix_socket.rs,
//    app-server-client/src/remote.rs]. `codex app-server proxy` only relays raw bytes, so the
//    client performs the HTTP upgrade and RFC 6455 framing itself over the proxy's stdio.

import { createHash, randomBytes } from "node:crypto";
import { StringDecoder } from "node:string_decoder";

export interface CodecHooks {
  write(data: Buffer): void;
  onMessage(text: string): void;
  onNoise(text: string): void;
  /** The peer closed the channel at the framing level (WebSocket close frame). */
  onPeerClose?(reason: string): void;
}

export interface MessageCodec {
  /** Prepares the channel (no-op for JSONL, HTTP upgrade for WebSocket). */
  open(): Promise<void>;
  /** Raw bytes from the child's stdout. */
  push(chunk: Buffer): void;
  /** Frames and writes one message. */
  send(message: string): void;
  /** Best-effort polite shutdown at the framing level. */
  close(): void;
}

export type CodecKind = "jsonl" | "websocket";

const DEFAULT_MAX_MESSAGE_BYTES = 16 * 1024 * 1024;

export class JsonlCodec implements MessageCodec {
  #hooks: CodecHooks;
  #decoder = new StringDecoder("utf8");
  #buffer = "";
  #maxLength: number;

  constructor(hooks: CodecHooks, maxMessageBytes = DEFAULT_MAX_MESSAGE_BYTES) {
    this.#hooks = hooks;
    this.#maxLength = maxMessageBytes;
  }

  open(): Promise<void> {
    return Promise.resolve();
  }

  push(chunk: Buffer): void {
    this.#buffer += this.#decoder.write(chunk);
    let newline: number;
    while ((newline = this.#buffer.indexOf("\n")) >= 0) {
      const line = this.#buffer.slice(0, newline).replace(/\r$/, "");
      this.#buffer = this.#buffer.slice(newline + 1);
      if (line.trim()) this.#hooks.onMessage(line);
    }
    if (this.#buffer.length > this.#maxLength) {
      this.#hooks.onNoise(`[dropped ${this.#buffer.length} chars without newline] ${this.#buffer.slice(0, 120)}`);
      this.#buffer = "";
    }
  }

  send(message: string): void {
    this.#hooks.write(Buffer.from(message + "\n", "utf8"));
  }

  close(): void {}
}

const WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

export function websocketAccept(key: string): string {
  return createHash("sha1").update(key + WS_GUID).digest("base64");
}

/** Minimal RFC 6455 client: text messages, fragmentation, ping/pong, close. No extensions. */
export class WebSocketCodec implements MessageCodec {
  #hooks: CodecHooks;
  #host: string;
  #path: string;
  #maxMessageBytes: number;
  #key = randomBytes(16).toString("base64");
  #handshakeDone = false;
  #handshakeBuffer: Buffer = Buffer.alloc(0);
  #frameBuffer: Buffer = Buffer.alloc(0);
  #fragments: Buffer[] = [];
  #fragmentOpcode = 0;
  #closeSent = false;
  #settleHandshake: { resolve: () => void; reject: (e: Error) => void } | null = null;

  constructor(hooks: CodecHooks, options: { host?: string; path?: string; maxMessageBytes?: number } = {}) {
    this.#hooks = hooks;
    this.#host = options.host ?? "localhost";
    this.#path = options.path ?? "/rpc";
    this.#maxMessageBytes = options.maxMessageBytes ?? DEFAULT_MAX_MESSAGE_BYTES;
  }

  open(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.#settleHandshake = { resolve, reject };
      const request =
        `GET ${this.#path} HTTP/1.1\r\n` +
        `Host: ${this.#host}\r\n` +
        "Upgrade: websocket\r\n" +
        "Connection: Upgrade\r\n" +
        `Sec-WebSocket-Key: ${this.#key}\r\n` +
        "Sec-WebSocket-Version: 13\r\n\r\n";
      this.#hooks.write(Buffer.from(request, "latin1"));
    });
  }

  push(chunk: Buffer): void {
    if (!this.#handshakeDone) {
      this.#handshakeBuffer = Buffer.concat([this.#handshakeBuffer, chunk]);
      const end = this.#handshakeBuffer.indexOf("\r\n\r\n");
      if (end < 0) {
        if (this.#handshakeBuffer.length > 16 * 1024) this.#failHandshake("handshake response too large");
        return;
      }
      const head = this.#handshakeBuffer.subarray(0, end).toString("latin1");
      const rest = this.#handshakeBuffer.subarray(end + 4);
      this.#handshakeBuffer = Buffer.alloc(0);
      const error = this.#checkHandshake(head);
      if (error) {
        this.#failHandshake(error);
        return;
      }
      this.#handshakeDone = true;
      this.#settleHandshake?.resolve();
      this.#settleHandshake = null;
      if (rest.length === 0) return;
      chunk = rest;
    }
    this.#frameBuffer = this.#frameBuffer.length ? Buffer.concat([this.#frameBuffer, chunk]) : chunk;
    this.#drainFrames();
  }

  send(message: string): void {
    if (!this.#handshakeDone) throw new Error("WebSocket handshake not complete");
    this.#hooks.write(encodeClientFrame(0x1, Buffer.from(message, "utf8")));
  }

  close(): void {
    if (!this.#handshakeDone || this.#closeSent) return;
    this.#closeSent = true;
    const payload = Buffer.alloc(2);
    payload.writeUInt16BE(1000, 0);
    try {
      this.#hooks.write(encodeClientFrame(0x8, payload));
    } catch {
      // the pipe may already be gone
    }
  }

  #checkHandshake(head: string): string | null {
    const [statusLine, ...headerLines] = head.split("\r\n");
    if (!/^HTTP\/1\.1 101\b/.test(statusLine ?? "")) return `upgrade refused: ${statusLine ?? "<empty>"}`;
    const headers = new Map<string, string>();
    for (const line of headerLines) {
      const i = line.indexOf(":");
      if (i > 0) headers.set(line.slice(0, i).trim().toLowerCase(), line.slice(i + 1).trim());
    }
    if (headers.get("sec-websocket-accept") !== websocketAccept(this.#key)) return "invalid Sec-WebSocket-Accept";
    return null;
  }

  #failHandshake(reason: string): void {
    this.#settleHandshake?.reject(new Error(`WebSocket handshake failed: ${reason}`));
    this.#settleHandshake = null;
  }

  #drainFrames(): void {
    for (;;) {
      const buf = this.#frameBuffer;
      if (buf.length < 2) return;
      const fin = (buf[0]! & 0x80) !== 0;
      const opcode = buf[0]! & 0x0f;
      const masked = (buf[1]! & 0x80) !== 0;
      let length = buf[1]! & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (buf.length < 4) return;
        length = buf.readUInt16BE(2);
        offset = 4;
      } else if (length === 127) {
        if (buf.length < 10) return;
        const big = buf.readBigUInt64BE(2);
        if (big > BigInt(this.#maxMessageBytes)) {
          this.#protocolError(`frame of ${big} bytes exceeds limit`);
          return;
        }
        length = Number(big);
        offset = 10;
      }
      const maskOffset = offset;
      if (masked) offset += 4;
      if (buf.length < offset + length) return;
      let payload = buf.subarray(offset, offset + length);
      if (masked) {
        const mask = buf.subarray(maskOffset, maskOffset + 4);
        payload = Buffer.from(payload.map((b, i) => b ^ mask[i % 4]!));
      }
      this.#frameBuffer = buf.subarray(offset + length);
      this.#handleFrame(fin, opcode, payload);
    }
  }

  #handleFrame(fin: boolean, opcode: number, payload: Buffer): void {
    switch (opcode) {
      case 0x0: // continuation
        if (this.#fragmentOpcode === 0) return this.#protocolError("unexpected continuation frame");
        this.#fragments.push(payload);
        if (fin) this.#finishMessage();
        return;
      case 0x1: // text
      case 0x2: // binary (not expected; surfaced as noise)
        this.#fragmentOpcode = opcode;
        this.#fragments = [payload];
        if (fin) this.#finishMessage();
        return;
      case 0x8: {
        const code = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
        this.close();
        this.#hooks.onPeerClose?.(`close frame ${code} ${payload.subarray(2).toString("utf8")}`.trim());
        return;
      }
      case 0x9: // ping -> pong with the same payload
        try {
          this.#hooks.write(encodeClientFrame(0xa, payload));
        } catch {
          // channel already closing
        }
        return;
      case 0xa: // pong
        return;
      default:
        this.#protocolError(`unknown opcode ${opcode}`);
    }
  }

  #finishMessage(): void {
    const data = Buffer.concat(this.#fragments);
    const opcode = this.#fragmentOpcode;
    this.#fragments = [];
    this.#fragmentOpcode = 0;
    if (data.length > this.#maxMessageBytes) return this.#hooks.onNoise(`[dropped ${data.length}-byte message]`);
    if (opcode === 0x1) this.#hooks.onMessage(data.toString("utf8"));
    else this.#hooks.onNoise(`[binary frame, ${data.length} bytes]`);
  }

  #protocolError(reason: string): void {
    this.#frameBuffer = Buffer.alloc(0);
    this.#hooks.onNoise(`[websocket protocol error: ${reason}]`);
    this.close();
    this.#hooks.onPeerClose?.(`protocol error: ${reason}`);
  }
}

/** Client-to-server frames must be masked (RFC 6455 section 5.3). */
export function encodeClientFrame(opcode: number, payload: Buffer): Buffer {
  const mask = randomBytes(4);
  let header: Buffer;
  if (payload.length < 126) {
    header = Buffer.from([0x80 | opcode, 0x80 | payload.length]);
  } else if (payload.length < 65536) {
    header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 0x80 | 126;
    header.writeUInt16BE(payload.length, 2);
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 0x80 | 127;
    header.writeBigUInt64BE(BigInt(payload.length), 2);
  }
  const body = Buffer.alloc(payload.length);
  for (let i = 0; i < payload.length; i++) body[i] = payload[i]! ^ mask[i % 4]!;
  return Buffer.concat([header, mask, body]);
}

export function createCodec(kind: CodecKind, hooks: CodecHooks): MessageCodec {
  return kind === "websocket" ? new WebSocketCodec(hooks) : new JsonlCodec(hooks);
}
