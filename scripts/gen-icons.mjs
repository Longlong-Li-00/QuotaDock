// Generates QuotaDock's application icons (no dependencies): src-tauri/icons/icon.ico with
// 16-256 px images, plus 32x32.png and 128x128.png. With `tray`, it writes only the neutral tray
// icon, icons/tray/normal.png, and leaves the app icons alone. The other tray icons are separate.
// usage: node scripts/gen-icons.mjs [tray]
//
// Design: a dark rounded square with two quota bars (Codex, Claude) standing on a dock line.
// Shapes are rounded rectangles in a 0..1 unit square, rendered with 8x8 supersampling.
import { writeFileSync } from "node:fs";
import { deflateSync } from "node:zlib";
import { join } from "node:path";

const OUT = join(import.meta.dirname, "..", "src-tauri", "icons");

const rgba = (hex) => [parseInt(hex.slice(1, 3), 16), parseInt(hex.slice(3, 5), 16), parseInt(hex.slice(5, 7), 16)];
// [x0, y0, x1, y1, radius, colour]; painted in order.
const SHAPES = [
  [0.04, 0.04, 0.96, 0.96, 0.2, "#14324f"],
  [0.25, 0.22, 0.45, 0.68, 0.06, "#34c27a"],
  [0.55, 0.4, 0.75, 0.68, 0.06, "#8fdcb7"],
  [0.18, 0.73, 0.82, 0.8, 0.035, "#e6eef5"],
];
// The tray's resting state: the same two bars, neutral, no tile. Edges sit on even pixels of the
// 32 px image so they stay sharp when Windows halves it to 16 px.
const TRAY_SIZE = 32;
const TRAY_SHAPES = [
  [6 / 32, 4 / 32, 14 / 32, 28 / 32, 1.5 / 32, "#767676"],
  [18 / 32, 14 / 32, 26 / 32, 28 / 32, 1.5 / 32, "#767676"],
];

// Signed distance to a rounded rectangle (negative inside).
function sdRoundRect(px, py, [x0, y0, x1, y1, r]) {
  const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2, hx = (x1 - x0) / 2 - r, hy = (y1 - y0) / 2 - r;
  const qx = Math.abs(px - cx) - hx, qy = Math.abs(py - cy) - hy;
  return Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) + Math.min(Math.max(qx, qy), 0) - r;
}

function render(size, shapes = SHAPES) {
  const px = new Float64Array(size * size * 4); // premultiplied RGBA, 0..1
  const N = 8;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let [r, g, b, a] = [0, 0, 0, 0];
      for (const s of shapes) {
        let cover = 0;
        for (let j = 0; j < N; j++) for (let i = 0; i < N; i++) if (sdRoundRect((x + (i + 0.5) / N) / size, (y + (j + 0.5) / N) / size, s) <= 0) cover++;
        cover /= N * N;
        if (!cover) continue;
        const [cr, cg, cb] = rgba(s[5]).map((v) => v / 255);
        r = cr * cover + r * (1 - cover);
        g = cg * cover + g * (1 - cover);
        b = cb * cover + b * (1 - cover);
        a = cover + a * (1 - cover);
      }
      px.set([r, g, b, a], (y * size + x) * 4);
    }
  }
  // Straight (non-premultiplied) 8-bit RGBA.
  const out = Buffer.alloc(size * size * 4);
  for (let i = 0; i < size * size; i++) {
    const a = px[i * 4 + 3];
    for (let c = 0; c < 3; c++) out[i * 4 + c] = a ? Math.round((px[i * 4 + c] / a) * 255) : 0;
    out[i * 4 + 3] = Math.round(a * 255);
  }
  return out;
}

const CRC = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});
function crc32(buf) {
  let c = -1;
  for (const byte of buf) c = CRC[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4), crc = Buffer.alloc(4), td = Buffer.concat([Buffer.from(type), data]);
  len.writeUInt32BE(data.length);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
function png(size, pixels) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr.set([8, 6, 0, 0, 0], 8);
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) pixels.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw, { level: 9 })), chunk("IEND", Buffer.alloc(0))]);
}
// 32-bit DIB (bottom-up BGRA plus a 1-bit AND mask), the most compatible form for small icon images.
function dib(size, pixels) {
  const maskRow = Math.ceil(size / 32) * 4;
  const head = Buffer.alloc(40);
  head.writeUInt32LE(40, 0);
  head.writeInt32LE(size, 4);
  head.writeInt32LE(size * 2, 8);
  head.writeUInt16LE(1, 12);
  head.writeUInt16LE(32, 14);
  head.writeUInt32LE(size * size * 4 + maskRow * size, 20);
  const body = Buffer.alloc(size * size * 4), mask = Buffer.alloc(maskRow * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const s = (y * size + x) * 4, d = ((size - 1 - y) * size + x) * 4;
      body.set([pixels[s + 2], pixels[s + 1], pixels[s], pixels[s + 3]], d);
      if (pixels[s + 3] === 0) mask[(size - 1 - y) * maskRow + (x >> 3)] |= 0x80 >> (x & 7);
    }
  }
  return Buffer.concat([head, body, mask]);
}

if (process.argv[2] === "tray") {
  writeFileSync(join(OUT, "tray", "normal.png"), png(TRAY_SIZE, render(TRAY_SIZE, TRAY_SHAPES)));
  console.log(`normal.png (${TRAY_SIZE} px) -> ${join(OUT, "tray")}`);
  process.exit(0);
}

const ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256];
const images = ICO_SIZES.map((s) => {
  const p = render(s);
  // 256 px as PNG (the usual Vista+ form), the rest as DIBs for older readers.
  return { s, p, data: s >= 256 ? png(s, p) : dib(s, p) };
});
const dir = Buffer.alloc(6 + 16 * images.length);
dir.writeUInt16LE(1, 2);
dir.writeUInt16LE(images.length, 4);
let offset = dir.length;
images.forEach(({ s, data }, i) => {
  const e = 6 + 16 * i;
  dir[e] = s >= 256 ? 0 : s;
  dir[e + 1] = s >= 256 ? 0 : s;
  dir.writeUInt16LE(1, e + 4);
  dir.writeUInt16LE(32, e + 6);
  dir.writeUInt32LE(data.length, e + 8);
  dir.writeUInt32LE(offset, e + 12);
  offset += data.length;
});
writeFileSync(join(OUT, "icon.ico"), Buffer.concat([dir, ...images.map((i) => i.data)]));
for (const s of [32, 128]) writeFileSync(join(OUT, `${s}x${s}.png`), png(s, s === 32 ? images.find((i) => i.s === 32).p : render(s)));
console.log(`icon.ico (${ICO_SIZES.join(", ")} px), 32x32.png, 128x128.png -> ${OUT}`);
