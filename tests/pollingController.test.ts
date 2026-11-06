import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  backoffDelayMs,
  DEFAULT_INTERVAL_MS,
  PollingController,
  validateIntervalMs,
  type PollEvent,
  type Scheduler,
} from "../src/codex/polling/pollingController.ts";

class FakeScheduler implements Scheduler {
  time = 0;
  timers: Array<{ id: number; at: number; fn: () => void; delay: number }> = [];
  #id = 0;
  setTimeout(fn: () => void, ms: number): unknown {
    const id = ++this.#id;
    this.timers.push({ id, at: this.time + ms, fn, delay: ms });
    return id;
  }
  clearTimeout(handle: unknown): void {
    this.timers = this.timers.filter((t) => t.id !== handle);
  }
  now(): number {
    return this.time;
  }
  /** Advances to the next timer and fires it. */
  fireNext(): number {
    const next = this.timers.sort((a, b) => a.at - b.at).shift();
    if (!next) throw new Error("no timer scheduled");
    this.time = next.at;
    next.fn();
    return next.delay;
  }
}

const flush = () => new Promise((r) => setImmediate(r));

describe("interval validation", () => {
  it("defaults to 5 minutes and rejects intervals under 30 s", () => {
    assert.equal(DEFAULT_INTERVAL_MS, 300_000);
    assert.equal(validateIntervalMs(29_000).ok, false);
    assert.deepEqual(validateIntervalMs(30_000), { ok: true, intervalMs: 30_000, developmentOnly: true });
    assert.deepEqual(validateIntervalMs(60_000), { ok: true, intervalMs: 60_000, developmentOnly: true });
    assert.deepEqual(validateIntervalMs(300_000), { ok: true, intervalMs: 300_000, developmentOnly: false });
  });

  it("--unsafe-dev-poll lowers the floor to 10 s, never to per-second polling", () => {
    assert.equal(validateIntervalMs(10_000, true).ok, true);
    assert.equal(validateIntervalMs(5_000, true).ok, false);
    assert.equal(validateIntervalMs(1_000, true).ok, false);
    assert.equal(validateIntervalMs(Number.NaN).ok, false);
  });
});

describe("backoff", () => {
  it("steps 1, 2, 5, 10 minutes and caps at 15", () => {
    assert.deepEqual([1, 2, 3, 4, 5, 6, 20].map((n) => backoffDelayMs(n) / 60_000), [1, 2, 5, 10, 15, 15, 15]);
    assert.equal(backoffDelayMs(0), 0);
  });
});

describe("PollingController", () => {
  it("backs off on failures and returns to the normal interval after a success", async () => {
    const sched = new FakeScheduler();
    const outcomes = ["fail", "fail", "fail", "ok", "ok"];
    const events: Array<PollEvent<string>> = [];
    const c = new PollingController<string>({
      tick: async () => outcomes.shift() ?? "ok",
      classify: (r) => (r === "ok" ? "success" : "failure"),
      onEvent: (e) => events.push(e),
      intervalMs: 300_000,
      scheduler: sched,
    });
    c.start();
    await flush();
    const delays: number[] = [];
    for (let i = 0; i < 4; i++) {
      delays.push(sched.fireNext());
      await flush();
    }
    // first tick runs immediately; scheduled delays after each tick:
    assert.deepEqual(events.map((e) => e.nextDelayMs), [60_000, 120_000, 300_000, 300_000, 300_000]);
    assert.deepEqual(delays, [60_000, 120_000, 300_000, 300_000]);
    assert.equal(events.at(-1)?.consecutiveFailures, 0);
    c.stop();
  });

  it("treats a thrown tick as a failure", async () => {
    const sched = new FakeScheduler();
    const events: Array<PollEvent<string>> = [];
    const c = new PollingController<string>({
      tick: () => Promise.reject(new Error("boom")),
      classify: () => "success",
      onEvent: (e) => events.push(e),
      scheduler: sched,
    });
    c.start();
    await flush();
    assert.equal(events[0]?.outcome, "failure");
    assert.equal(events[0]?.nextDelayMs, 60_000);
    c.stop();
  });

  it("coalesces and throttles manual refreshes", async () => {
    const sched = new FakeScheduler();
    let release: (() => void) | null = null;
    let ticks = 0;
    const c = new PollingController<string>({
      tick: () => {
        ticks++;
        return new Promise<string>((r) => {
          release = () => r("ok");
        });
      },
      classify: () => "success",
      onEvent: () => {},
      scheduler: sched,
    });
    c.start();
    await flush();
    assert.equal(c.refreshNow(), "coalesced", "a read is already in flight");
    release!();
    await flush();
    sched.time += 5_000;
    assert.equal(c.refreshNow(), "throttled", "less than 10 s since the last read started");
    sched.time += 6_000;
    assert.equal(c.refreshNow(), "started");
    await flush();
    assert.equal(ticks, 2);
    release!();
    await flush();
    c.stop();
  });

  it("stop() prevents further scheduling", async () => {
    const sched = new FakeScheduler();
    const c = new PollingController<string>({ tick: async () => "ok", classify: () => "success", onEvent: () => {}, scheduler: sched });
    c.start();
    c.stop();
    await flush();
    assert.equal(sched.timers.length, 0);
    assert.equal(c.running, false);
  });
});
