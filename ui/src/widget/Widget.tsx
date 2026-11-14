import { useCallback, useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent, type PointerEvent } from "react";
import type { DisplayTone, WidgetView } from "../api/types.ts";
import type { Dict } from "../i18n/en.ts";
import { dictFor } from "../i18n/index.ts";
import { widgetApi } from "./api.ts";
import { barWidth, headerStatus, ringView, rowView, type RingView, type RowView } from "./present.ts";

declare global {
  interface Window {
    __quotadockWidget?: { refresh: () => void };
  }
}

/** Safety-net re-read (local data only); the backend also nudges `refresh` after every update. */
const VIEW_REFRESH_MS = 60_000;
/** "N min ago" labels only; no backend request. */
const CLOCK_MS = 15_000;
/** Pointer movement (CSS px) that turns a press into a drag; less is a click (which does nothing). */
const DRAG_THRESHOLD = 4;

const openOnKey = (e: KeyboardEvent) => {
  if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    void widgetApi.openMain();
  }
};

export function Widget() {
  const [view, setView] = useState<WidgetView | null>(null);
  const [now, setNow] = useState(Date.now());
  const press = useRef<{ x: number; y: number } | null>(null);

  const reload = useCallback(async () => {
    try {
      setView(await widgetApi.getView());
    } catch {
      // Keep the last view; the next nudge or timer tries again.
    }
    setNow(Date.now());
  }, []);

  useEffect(() => {
    window.__quotadockWidget = { refresh: () => void reload() };
    void reload();
    const visible = () => document.visibilityState === "visible";
    const timer = setInterval(() => visible() && void reload(), VIEW_REFRESH_MS);
    const clock = setInterval(() => visible() && setNow(Date.now()), CLOCK_MS);
    const onVisibility = () => visible() && void reload();
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearInterval(timer);
      clearInterval(clock);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [reload]);

  const lang = view?.language;
  const theme = view?.theme;
  useEffect(() => {
    if (!lang || !theme) return;
    document.documentElement.lang = lang;
    if (theme === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }, [lang, theme]);

  // Press anywhere and move: drag (the backend refuses while locked). Double-click on the header or a
  // provider row: open QuotaDock. A single click does nothing, so brushing the widget never opens it.
  const onPointerDown = (e: PointerEvent) => {
    press.current = e.button === 0 ? { x: e.clientX, y: e.clientY } : null;
  };
  const onPointerMove = (e: PointerEvent) => {
    const p = press.current;
    if (!p || (e.buttons & 1) === 0) return;
    if (Math.abs(e.clientX - p.x) + Math.abs(e.clientY - p.y) < DRAG_THRESHOLD) return;
    press.current = null;
    void widgetApi.startDrag();
  };
  const onPointerUp = () => {
    press.current = null;
  };
  const onDoubleClick = (e: MouseEvent) => {
    if (e.button === 0 && (e.target as Element).closest("[data-open]") !== null) void widgetApi.openMain();
  };
  const onContextMenu = (e: MouseEvent) => {
    e.preventDefault();
    press.current = null;
    void widgetApi.contextMenu();
  };
  const handlers = {
    onPointerDown,
    onPointerMove,
    onPointerUp,
    onPointerCancel: () => {
      press.current = null;
    },
    onDoubleClick,
    onContextMenu,
  };

  if (!view) return <div className="w" {...handlers} />;
  const t = dictFor(view.language);
  const head = headerStatus(view, now, t);
  const rows = view.providers.map((p) => rowView(p, now, t, view.language));
  const rings = view.mode === "detailed" ? view.providers.map((p) => ringView(p, now, t, view.language)) : [];
  const style = { "--w-alpha": String(view.opacity / 100) } as CSSProperties;
  return (
    <div className={`w w-${view.mode}${view.opacity < 100 ? " w-see-through" : ""}`} style={style} {...handlers}>
      {rows.length > 0 && view.mode === "compact" && (
        <div className="w-head" data-open role="button" tabIndex={0} title={t.wOpen} aria-label={t.wOpen} onKeyDown={openOnKey}>
          <span className="w-label">{t.wRemainingHead}</span>
          {head && <span className={`w-status w-tone-${head.tone}`}>{head.text}</span>}
          <span className="w-cols" aria-hidden="true">
            {view.providers.some((p) => p.showFiveHour) && <span>{t.wFiveHour}</span>}
            <span>{t.wWeekly}</span>
          </span>
        </div>
      )}
      {view.mode === "compact"
        ? rows.map((r) => <CompactRow key={r.id} r={r} />)
        : rings.map((r, i) => [i > 0 && <div key={`sep-${r.id}`} className="w-ring-sep" />, <RingRow key={r.id} r={r} icon={view.customIcons[r.id]} t={t} />])}
      {rows.length === 0 && (
        <div className="w-empty" data-open role="button" tabIndex={0} title={t.wOpen} aria-label={t.wOpen} onKeyDown={openOnKey}>
          {t.wNoProviders}
        </div>
      )}
    </div>
  );
}

function CompactRow({ r }: { r: RowView }) {
  return (
    <div className={`w-row${r.showFive ? "" : " w-no-five"}`} data-open role="button" tabIndex={0} title={r.label} aria-label={r.label} onKeyDown={openOnKey}>
      <span className={`w-name${r.wide ? " w-wide" : ""}`}>
        {r.name}
        {r.note && <span className={`w-note w-tone-${r.note.tone}`}>{r.note.text}</span>}
      </span>
      {!r.wide && (
        <>
          {r.showFive && <span className={`w-val w-t-${r.fiveTone}`}>{r.five}</span>}
          <span className={`w-val w-t-${r.weeklyTone}`}>{r.weekly}</span>
        </>
      )}
    </div>
  );
}

/** Weekly on the outer ring and in the big number; 5-hour on the inner ring and its own line. */
function RingRow({ r, icon, t }: { r: RingView; icon: string | undefined; t: Dict }) {
  return (
    <div className={`w-ring${r.five ? " w-ring-five" : ""}`} data-open role="button" tabIndex={0} title={r.label} aria-label={r.label} onKeyDown={openOnKey}>
      <Gauge r={r} icon={icon} />
      <div className="w-ring-text" aria-hidden="true">
        <div className="w-ring-line">
          <span className="w-ring-name">{r.name}</span>
          <span className="w-ring-sub"> · {t.weekly}</span>
        </div>
        <div className={`w-ring-value w-t-${r.weeklyTone}`}>
          <span className="w-ring-num">{r.weekly}</span>
          {r.weeklyValue !== null && <span className="w-ring-sub">{t.wRingLeft}</span>}
        </div>
        <div className={`w-ring-line w-tone-${r.line.tone}`}>{r.line.text}</div>
        {r.five && (
          <div className="w-ring-line w-tone-muted">
            {t.fiveHour} <span className={`w-ring-five-val w-t-${r.five.tone}`}>{r.five.text}</span>
            {r.five.reset && ` · ${r.five.reset}`}
          </div>
        )}
      </div>
    </div>
  );
}

/** Ring geometry in a 52 px box: the two rings, or one thicker ring when 5-hour is hidden, share the outer edge (radius 26). */
const GAUGE = 52;
const OUTER = { r: 23.5, w: 5 };
const SINGLE = { r: 22, w: 8 };
const INNER = { r: 17.25, w: 3.5 };

function Gauge({ r, icon }: { r: RingView; icon: string | undefined }) {
  const outer = r.five ? OUTER : SINGLE;
  return (
    <span className="w-ring-gauge" aria-hidden="true">
      <svg width={GAUGE} height={GAUGE} viewBox={`0 0 ${GAUGE} ${GAUGE}`}>
        <g transform={`rotate(-90 ${GAUGE / 2} ${GAUGE / 2})`} fill="none" strokeLinecap="round">
          <Arc ring={outer} value={r.weeklyValue} tone={r.weeklyTone} />
          {r.five && <Arc ring={INNER} value={r.five.value} tone={r.five.tone} inner />}
        </g>
      </svg>
      <span className={`w-ring-icon w-t-${r.weeklyTone}`}>{icon ? <img src={icon} alt="" draggable={false} /> : <BuiltInIcon id={r.id} />}</span>
    </span>
  );
}

function Arc({ ring, value, tone, inner = false }: { ring: { r: number; w: number }; value: number | null; tone: DisplayTone; inner?: boolean }) {
  const c = 2 * Math.PI * ring.r;
  const part = barWidth(value) / 100;
  // Like a phone's battery: a low ring tints its track with its tone, and an empty one (limit reached,
  // nothing to draw) is the whole ring in that tone, so the state reads from the ring alone.
  const alert = tone === "caution" || tone === "low" || tone === "critical" || tone === "empty";
  const track = `w-ring-track${alert ? (part > 0 ? " w-ring-track-alert" : " w-ring-track-empty") : ""}`;
  return (
    <g className={`w-t-${tone}${inner ? " w-ring-inner" : ""}`}>
      <circle className={track} cx={GAUGE / 2} cy={GAUGE / 2} r={ring.r} strokeWidth={ring.w} />
      {part > 0 && <circle className="w-ring-fill" cx={GAUGE / 2} cy={GAUGE / 2} r={ring.r} strokeWidth={ring.w} strokeDasharray={c} strokeDashoffset={c * (1 - part)} />}
    </g>
  );
}

/** QuotaDock's own plain icons (no company's mark): code brackets for Codex, a speech bubble for Claude. */
function BuiltInIcon({ id }: { id: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      {id === "codex" ? (
        <path d="M8 7l-5 5 5 5M16 7l5 5-5 5" />
      ) : (
        <path d="M12 4c4.4 0 8 3 8 7s-3.6 7-8 7c-1 0-2-.15-2.9-.45L5 19l1.1-3.3C4.8 14.4 4 12.8 4 11c0-4 3.6-7 8-7z" />
      )}
    </svg>
  );
}
