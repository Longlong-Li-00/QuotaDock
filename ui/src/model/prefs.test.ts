import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { DEFAULT_HISTORY_RANGE, loadHistoryRange, saveHistoryRange } from "./prefs.ts";

function memory() {
  const m = new Map<string, string>();
  return { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
}

describe("remembered history range", () => {
  it("opens on 12 hours the first time", () => {
    assert.equal(DEFAULT_HISTORY_RANGE, "twelve_hours");
    assert.equal(loadHistoryRange(memory()), "twelve_hours");
  });

  it("remembers the last choice", () => {
    const s = memory();
    saveHistoryRange("six_hours", s);
    assert.equal(loadHistoryRange(s), "six_hours");
    saveHistoryRange("week", s);
    assert.equal(loadHistoryRange(s), "week");
  });

  it("ignores an unknown stored value and works without storage", () => {
    const s = memory();
    s.setItem("quotadock.history.range", "fortnight");
    assert.equal(loadHistoryRange(s), "twelve_hours");
    assert.equal(loadHistoryRange(null), "twelve_hours");
    const broken = { getItem: () => { throw new Error("denied"); }, setItem: () => { throw new Error("denied"); } };
    assert.equal(loadHistoryRange(broken), "twelve_hours");
    assert.doesNotThrow(() => saveHistoryRange("day", broken));
  });
});
