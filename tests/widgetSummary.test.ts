import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { summarizeWidget, type HealthLine } from "../scripts/widget-summary.ts";

const H = 3_600_000;
const T0 = Date.UTC(2026, 9, 18, 7, 30, 0);

const widget = (t: number, event: string, extra: Record<string, unknown> = {}): HealthLine => ({ t, kind: "widget", event, widget: 1, ...extra });
const fg = (t: number, value: boolean): HealthLine => ({ t, kind: "foreground", value });

describe("desktop widget soak summary", () => {
  it("no widget activity: nothing to report", () => {
    assert.equal(summarizeWidget([{ t: T0, kind: "app_start" }, fg(T0 + 1, false)], T0, T0 + H), null);
  });

  it("measures time shown, and shown while the main window is hidden", () => {
    const lines = [
      { t: T0, kind: "app_start" },
      fg(T0 + 1_000, true),
      widget(T0 + 2_000, "created", { alwaysOnTop: true, restored: true }),
      fg(T0 + 2_000 + H, false), // main hidden after 1 h
      widget(T0 + 2_000 + 3 * H, "destroyed", { reason: "quit", widget: 1 }),
      { t: T0 + 2_100 + 3 * H, kind: "app_stop" },
    ];
    const s = summarizeWidget(lines, T0, T0 + 4 * H)!;
    assert.equal(s.shownMs, 3 * H);
    assert.equal(s.shownMainHiddenMs, 2 * H);
    assert.deepEqual(s.destroyedBy, { quit: 1 });
    assert.deepEqual(s.placements, { restored: 1, recovered: 0, default: 0 });
    assert.deepEqual(s.alwaysOnTop, { on: 1, off: 0 });
    assert.equal(s.createdWhileOpen, 0);
    assert.equal(s.maxWidgetWindows, 1);
  });

  it("flags a second 'created' while one is open; a restart is not a second widget", () => {
    const lines = [
      widget(T0 + 1, "created"),
      widget(T0 + 2, "created"), // would be a single-instance violation
      { t: T0 + 3, kind: "app_start" }, // crash and restart: the old widget is gone
      widget(T0 + 4, "created", { recovered: true }),
    ];
    const s = summarizeWidget(lines, T0, T0 + 10)!;
    assert.equal(s.createdWhileOpen, 1);
    assert.deepEqual(s.placements, { restored: 0, recovered: 1, default: 2 });
  });

  it("a widget opened before the report window counts from the window start", () => {
    const lines = [widget(T0 - H, "created"), fg(T0 - H, false), widget(T0 + H, "updated", { alwaysOnTop: false }), widget(T0 + 2 * H, "destroyed", { reason: "disabled" })];
    const s = summarizeWidget(lines, T0, T0 + 3 * H)!;
    assert.equal(s.shownMs, 2 * H);
    assert.equal(s.shownMainHiddenMs, 2 * H);
    assert.deepEqual(s.events, { updated: 1, destroyed: 1 });
    assert.deepEqual(s.alwaysOnTop, { on: 0, off: 1 });
    assert.deepEqual(s.destroyedBy, { disabled: 1 });
  });
});
