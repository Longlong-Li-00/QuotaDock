// Schedules background reads. `account/rateLimits/read` is a live backend request on every call
// (no app-server cache), so polling is deliberately slow and backs off on failure.

export const DEFAULT_INTERVAL_MS = 5 * 60_000;
/** Lowest interval accepted without --unsafe-dev-poll. */
export const MIN_SAFE_INTERVAL_MS = 30_000;
/** Absolute floor even with --unsafe-dev-poll. */
export const UNSAFE_FLOOR_MS = 10_000;
/** 1, 2, 5, 10, then 15 minutes (the cap). */
export const DEFAULT_BACKOFF_MS = [60_000, 120_000, 300_000, 600_000, 900_000];
export const MAX_BACKOFF_MS = 15 * 60_000;
/** Manual refreshes closer together than this are coalesced into the previous one. */
export const MIN_MANUAL_GAP_MS = 10_000;

export interface IntervalCheck {
  ok: boolean;
  intervalMs: number;
  /** Anything faster than the 5-minute default is for development only. */
  developmentOnly: boolean;
  error?: string;
}

export function validateIntervalMs(requestedMs: number, allowUnsafeDevPoll = false): IntervalCheck {
  if (!Number.isFinite(requestedMs) || requestedMs <= 0) {
    return { ok: false, intervalMs: DEFAULT_INTERVAL_MS, developmentOnly: false, error: "interval must be a positive number of seconds" };
  }
  const floor = allowUnsafeDevPoll ? UNSAFE_FLOOR_MS : MIN_SAFE_INTERVAL_MS;
  if (requestedMs < floor) {
    const hint = allowUnsafeDevPoll ? "" : " (pass --unsafe-dev-poll to go below 30 s, development only)";
    return { ok: false, intervalMs: DEFAULT_INTERVAL_MS, developmentOnly: true, error: `interval must be at least ${floor / 1000} s${hint}` };
  }
  return { ok: true, intervalMs: requestedMs, developmentOnly: requestedMs < DEFAULT_INTERVAL_MS };
}

export function backoffDelayMs(consecutiveFailures: number, schedule = DEFAULT_BACKOFF_MS, maxMs = MAX_BACKOFF_MS): number {
  if (consecutiveFailures <= 0) return 0;
  const step = schedule[Math.min(consecutiveFailures, schedule.length) - 1] ?? maxMs;
  return Math.min(step, maxMs);
}

export interface Scheduler {
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
  now(): number;
}

const realScheduler: Scheduler = {
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  now: () => Date.now(),
};

export type TickOutcome = "success" | "failure";

export interface PollEvent<T> {
  trigger: "scheduled" | "manual";
  result?: T;
  error?: unknown;
  outcome: TickOutcome;
  consecutiveFailures: number;
  nextDelayMs: number | null;
}

export interface PollingControllerOptions<T> {
  tick: () => Promise<T>;
  /** Decides whether a resolved result counts as success (e.g. status "ok"). */
  classify: (result: T) => TickOutcome;
  onEvent: (event: PollEvent<T>) => void;
  intervalMs?: number;
  backoffMs?: number[];
  scheduler?: Scheduler;
}

export class PollingController<T> {
  #opts: PollingControllerOptions<T>;
  #scheduler: Scheduler;
  #intervalMs: number;
  #timer: unknown = null;
  #running = false;
  #inFlight: Promise<void> | null = null;
  #lastTickStartedAt = Number.NEGATIVE_INFINITY;
  #consecutiveFailures = 0;

  constructor(options: PollingControllerOptions<T>) {
    this.#opts = options;
    this.#scheduler = options.scheduler ?? realScheduler;
    this.#intervalMs = options.intervalMs ?? DEFAULT_INTERVAL_MS;
  }

  get running(): boolean {
    return this.#running;
  }

  get consecutiveFailures(): number {
    return this.#consecutiveFailures;
  }

  /** Starts polling with an immediate first read. */
  start(): void {
    if (this.#running) return;
    this.#running = true;
    void this.#run("scheduled");
  }

  stop(): void {
    this.#running = false;
    this.#clearTimer();
  }

  /** Reads now unless a read is in flight or one started less than MIN_MANUAL_GAP_MS ago. */
  refreshNow(): "started" | "coalesced" | "throttled" {
    if (this.#inFlight) return "coalesced";
    if (this.#scheduler.now() - this.#lastTickStartedAt < MIN_MANUAL_GAP_MS) return "throttled";
    this.#clearTimer();
    void this.#run("manual");
    return "started";
  }

  /** Resolves when the current read (if any) has finished. */
  async idle(): Promise<void> {
    await this.#inFlight;
  }

  #run(trigger: "scheduled" | "manual"): Promise<void> {
    this.#lastTickStartedAt = this.#scheduler.now();
    const run = (async () => {
      await Promise.resolve(); // let #inFlight be assigned before tick() can settle
      let event: PollEvent<T>;
      try {
        const result = await this.#opts.tick();
        const outcome = this.#opts.classify(result);
        event = this.#settle(trigger, outcome, { result });
      } catch (error) {
        event = this.#settle(trigger, "failure", { error });
      }
      this.#inFlight = null;
      if (this.#running && event.nextDelayMs !== null) this.#schedule(event.nextDelayMs);
      this.#opts.onEvent(event);
    })();
    this.#inFlight = run;
    return run;
  }

  #settle(trigger: PollEvent<T>["trigger"], outcome: TickOutcome, payload: { result?: T; error?: unknown }): PollEvent<T> {
    this.#consecutiveFailures = outcome === "success" ? 0 : this.#consecutiveFailures + 1;
    const nextDelayMs = !this.#running
      ? null
      : outcome === "success"
        ? this.#intervalMs
        : backoffDelayMs(this.#consecutiveFailures, this.#opts.backoffMs);
    return { trigger, ...payload, outcome, consecutiveFailures: this.#consecutiveFailures, nextDelayMs };
  }

  #schedule(delayMs: number): void {
    this.#clearTimer();
    this.#timer = this.#scheduler.setTimeout(() => {
      this.#timer = null;
      if (this.#running) void this.#run("scheduled");
    }, delayMs);
  }

  #clearTimer(): void {
    if (this.#timer !== null) {
      this.#scheduler.clearTimeout(this.#timer);
      this.#timer = null;
    }
  }
}
