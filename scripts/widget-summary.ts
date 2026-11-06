// Desktop widget summary for the soak report (Phase 3.7 §72–§76), from health-log lines only:
// kind "widget" ({event, widget, reason, restored, recovered, alwaysOnTop}), "foreground"
// ({value}: the main window is shown) and "app_start" / "app_stop". Pure; no I/O.

export interface HealthLine {
  t: number;
  kind: string;
  [field: string]: unknown;
}

export interface WidgetSummary {
  events: Record<string, number>;
  destroyedBy: Record<string, number>;
  /** Highest widget window count reported with any widget event (must be 0 or 1). */
  maxWidgetWindows: number;
  /** "created" while a widget was already open in the same run (must be 0). */
  createdWhileOpen: number;
  /** Time a widget existed (it is never hidden: turning it off destroys it). */
  shownMs: number;
  /** ...of which the main window was hidden or minimized (the widget's main scenario). */
  shownMainHiddenMs: number;
  placements: { restored: number; recovered: number; default: number };
  alwaysOnTop: { on: number; off: number };
}

const isWidget = (h: HealthLine, event: string) => h.kind === "widget" && h.event === event;

/** Lines must be sorted by time; `since`/`now` bound the report window. Null: no widget activity. */
export function summarizeWidget(health: HealthLine[], since: number, now: number): WidgetSummary | null {
  const inWindow = health.filter((h) => h.t >= since && h.t <= now);
  const widgetLines = inWindow.filter((h) => h.kind === "widget");
  // State at the start of the window, from what happened before it.
  let open = false;
  let mainShown = false;
  for (const h of health) {
    if (h.t >= since) break;
    if (h.kind === "app_start" || h.kind === "app_stop") open = false;
    else if (h.kind === "foreground") mainShown = h.value === true;
    else if (isWidget(h, "created")) open = true;
    else if (isWidget(h, "destroyed")) open = false;
  }
  if (!widgetLines.length && !open) return null;

  const s: WidgetSummary = {
    events: {},
    destroyedBy: {},
    maxWidgetWindows: 0,
    createdWhileOpen: 0,
    shownMs: 0,
    shownMainHiddenMs: 0,
    placements: { restored: 0, recovered: 0, default: 0 },
    alwaysOnTop: { on: 0, off: 0 },
  };
  let last = since;
  const advance = (t: number) => {
    if (open) {
      s.shownMs += t - last;
      if (!mainShown) s.shownMainHiddenMs += t - last;
    }
    last = t;
  };
  for (const h of inWindow) {
    advance(h.t);
    if (h.kind === "app_start" || h.kind === "app_stop") {
      open = false; // a new run has no widget until it creates one
      if (h.kind === "app_start") mainShown = false;
    } else if (h.kind === "foreground") {
      mainShown = h.value === true;
    } else if (h.kind === "widget") {
      const event = String(h.event);
      s.events[event] = (s.events[event] ?? 0) + 1;
      s.maxWidgetWindows = Math.max(s.maxWidgetWindows, typeof h.widget === "number" ? h.widget : 0);
      if (event === "created") {
        if (open) s.createdWhileOpen++;
        open = true;
        if (h.recovered === true) s.placements.recovered++;
        else if (h.restored === true) s.placements.restored++;
        else s.placements.default++;
      } else if (event === "destroyed") {
        open = false;
        const reason = String(h.reason ?? "-");
        s.destroyedBy[reason] = (s.destroyedBy[reason] ?? 0) + 1;
      }
      if (event === "created" || event === "updated") {
        if (h.alwaysOnTop === true) s.alwaysOnTop.on++;
        else if (h.alwaysOnTop === false) s.alwaysOnTop.off++;
      }
    }
  }
  advance(now);
  return s;
}
