// The widget's see-through colours, read from the stylesheet: a quiet value must stay quiet below
// 100 %, and a value must clear 4.5:1 at the default opacity over flat white and black.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";

const widget = readFileSync(new URL("./widget.css", import.meta.url), "utf8");
const tokens = readFileSync(new URL("../tokens.css", import.meta.url), "utf8");
const DEFAULT_ALPHA = 0.85;

/** The declarations of the first rule with exactly this selector. */
function rule(css: string, selector: string): string {
  const at = css.indexOf(`${selector} {`);
  assert.notEqual(at, -1, selector);
  return css.slice(at + selector.length + 2, css.indexOf("}", at));
}

function token(block: string, name: string): string {
  const m = block.match(new RegExp(`${name}:\\s*([^;]+);`));
  assert.ok(m, name);
  return m[1]!.trim();
}

function rgb(hex: string): number[] {
  const m = hex.match(/^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i);
  assert.ok(m, hex);
  return [m[1]!, m[2]!, m[3]!].map((h) => parseInt(h, 16));
}

function luminance(c: number[]): number {
  const f = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * f(c[0]!) + 0.7152 * f(c[1]!) + 0.0722 * f(c[2]!);
}

function contrastAtDefault(text: string, surface: string): number {
  return Math.min(
    ...[0, 255].map((backdrop) => {
      const panel = rgb(surface).map((v) => v * DEFAULT_ALPHA + backdrop * (1 - DEFAULT_ALPHA));
      const [a, b] = [luminance(rgb(text)), luminance(panel)].sort((x, y) => y - x);
      return (a! + 0.05) / (b! + 0.05);
    }),
  );
}

const lightSeeThrough = rule(widget, ".w-see-through");
const darkSeeThrough = rule(widget, ':root[data-theme="dark"] .w-see-through');
const lightSurface = token(rule(tokens, ":root"), "--surface");
const darkSurface = token(rule(tokens, ':root[data-theme="dark"]'), "--surface");

describe("widget tones below 100 %", () => {
  it("a muted or unknown value reads its own token, which the see-through override leaves alone", () => {
    assert.match(rule(widget, ".w-t-muted, .w-t-unknown"), /--t: var\(--t-quiet\)/);
    assert.match(rule(widget, ".w-t-muted, .w-t-unknown"), /--t-weight: 400/);
    assert.match(lightSeeThrough, /--text-2: var\(--text\)/, "words keep the legibility rule");
    assert.equal(/--t-quiet:\s*var\(--text/.test(widget), false);
    const quiet = token(rule(widget, ":root"), "--t-quiet");
    assert.notEqual(quiet, token(rule(tokens, ":root"), "--text"));
  });

  it("a normal value is primary text, and so is its weekly ring; a normal 5-hour ring is quieter", () => {
    assert.match(rule(widget, ".w-t-normal"), /--t: var\(--text\);/);
    assert.match(rule(widget, ".w-ring-fill"), /stroke: var\(--t\)/);
    assert.match(rule(widget, ".w-ring-inner.w-t-normal"), /--t: var\(--ring-five\)/);
  });

  it("the system dark theme and the chosen dark theme say the same", () => {
    assert.equal(rule(widget, ':root:not([data-theme="light"]) .w-see-through'), darkSeeThrough);
  });

  it("every coloured value clears 4.5:1 at the default opacity over flat white and black", () => {
    const cases: [string, string, string][] = [
      ["light --warn", token(lightSeeThrough, "--warn"), lightSurface],
      ["light --crit", token(lightSeeThrough, "--crit"), lightSurface],
      ["light quiet", token(rule(widget, ":root"), "--t-quiet"), lightSurface],
      ["dark --warn", token(darkSeeThrough, "--warn"), darkSurface],
      ["dark --crit", token(darkSeeThrough, "--crit"), darkSurface],
      ["dark quiet", token(darkSeeThrough, "--t-quiet"), darkSurface],
    ];
    for (const [name, text, surface] of cases) {
      const c = contrastAtDefault(text, surface);
      assert.ok(c >= 4.5, `${name} ${text} over ${surface}: ${c.toFixed(2)}`);
    }
  });
});
