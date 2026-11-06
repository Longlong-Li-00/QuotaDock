// History as small multiples: one panel per window (5-hour, weekly), one solid line per provider
// (Codex blue, Claude orange), 0–100 %. Lines break at cycle / window-instance boundaries and gaps;
// spans without data are shaded; observed resets are small dots on the top edge. One crosshair,
// shared by both panels, reads every series at a time; a table view carries the same values
// without hovering. No chart library.

import { useState, type KeyboardEvent, type PointerEvent } from "react";
import type { HistoryData, HistoryPoint, WindowType } from "../api/types.ts";
import { useApp } from "../context.ts";
import { buildPaths, chartDomain, commonGaps, firstTime, pointAt, timeTicks, xOf, yOf, type Frame } from "../model/chart.ts";
import { eventTitle, formatDateTime, formatDay, formatTime } from "../model/present.ts";

type Series = HistoryData["series"][number];
type Marker = HistoryData["markers"][number];
type Domain = { t0: number; t1: number };

const PROVIDER_NAME: Record<string, string> = { codex: "Codex", claude: "Claude" };
const WINDOWS: WindowType[] = ["five_hour", "weekly"];
const MIN = 60_000;
const DAY = 24 * 60 * MIN;
const W = 760;
const H = 190;

export function HistoryChart({ data, mode }: { data: HistoryData; mode: "remaining" | "used" }) {
  const { t, lang } = useApp();
  const [hoverT, setHoverT] = useState<number | null>(null);
  const first = firstTime(data.series);
  const domain = chartDomain(data.fromMs, data.toMs, first);
  // A hover reads a point only this close to the pointer: two Claude updates, or the downsampling step.
  const tolerance = Math.max(20 * MIN, 1.5 * (data.resolutionMs ?? 0));
  const value = (p: HistoryPoint) => (mode === "remaining" ? p.remaining : p.used);
  const panels = WINDOWS.map((w) => ({ w, series: data.series.filter((s) => s.windowType === w && s.points.length > 0) })).filter((p) => p.series.length > 0);
  return (
    <div className="hc">
      {panels.map((p) => (
        <Panel
          key={p.w}
          window={p.w}
          series={p.series}
          markers={data.markers.filter((m) => m.windowType === p.w || m.windowType === null)}
          domain={domain}
          value={value}
          hoverT={hoverT}
          setHoverT={setHoverT}
          tolerance={tolerance}
        />
      ))}
      <p className="help">
        {domain.trimmed && first !== null && `${t.chartDataFrom(formatDateTime(first, lang))} `}
        {data.resolutionMs ? t.resolutionNote(Math.round(data.resolutionMs / MIN)) : t.rawNote} {t.segmentNote} {data.markers.length > 0 && `${t.chartMarkerHint} `}
        {t.chartHoverHint}
      </p>
      <HistoryTable series={panels.flatMap((p) => p.series)} domain={domain} value={value} tolerance={tolerance} />
    </div>
  );
}

function windowName(w: WindowType, t: ReturnType<typeof useApp>["t"]) {
  return w === "five_hour" ? t.fiveHourShort : t.weeklyShort;
}

function Panel(props: {
  window: WindowType;
  series: Series[];
  markers: Marker[];
  domain: Domain;
  value: (p: HistoryPoint) => number;
  hoverT: number | null;
  setHoverT: (t: number | null) => void;
  tolerance: number;
}) {
  const { window: win, series, markers, domain, value, hoverT, setHoverT, tolerance } = props;
  const { t, lang } = useApp();
  const f: Frame = { t0: domain.t0, t1: domain.t1, width: W, height: H, padLeft: 40, padRight: 16, padTop: 16, padBottom: 26 };
  const top = f.padTop;
  const bottom = H - f.padBottom;
  const span = f.t1 - f.t0;
  const inRange = (tm: number) => tm >= f.t0 && tm <= f.t1;
  const tickLabel = (tk: number) => (span > 2 * DAY ? formatDay(tk, lang) : formatTime(tk, lang));
  // Shaded only where no line of this panel has data.
  const gaps = commonGaps(series.map((s) => s.points))
    .map(([a, b]): [number, number] => [Math.max(a, f.t0), Math.min(b, f.t1)])
    .filter(([a, b]) => b > a);

  // The crosshair snaps to the nearest data time of this panel.
  const hits = hoverT === null ? [] : series.map((s) => ({ s, p: pointAt(s.points, hoverT, tolerance) }));
  let crossT: number | null = hoverT;
  for (const h of hits) if (h.p && hoverT !== null && (crossT === hoverT || Math.abs(h.p.t - hoverT) < Math.abs(crossT! - hoverT))) crossT = h.p.t;

  const timeAt = (e: PointerEvent<SVGSVGElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const frac = (((e.clientX - r.left) / r.width) * W - f.padLeft) / (W - f.padLeft - f.padRight);
    return frac < 0 || frac > 1 ? null : f.t0 + frac * span;
  };
  const onKey = (e: KeyboardEvent<SVGSVGElement>) => {
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      e.preventDefault();
      const step = (e.key === "ArrowLeft" ? -1 : 1) * (span / 48);
      setHoverT(Math.min(f.t1, Math.max(f.t0, (hoverT ?? f.t1) + step)));
    } else if (e.key === "Escape") {
      setHoverT(null);
    }
  };
  const names = series.map((s) => `${PROVIDER_NAME[s.provider] ?? s.provider} ${windowName(win, t)}`).join(", ");
  const xPct = crossT === null ? 0 : (xOf(crossT, f) / W) * 100;

  return (
    <figure className="hc-panel">
      <figcaption className="hc-head">
        <span className="hc-title">{windowName(win, t)}</span>
        <span className="hc-legend">
          <span className="muted">{t.chartLatest}</span>
          {series.map((s) => (
            <span key={s.provider} className="hc-legend-item">
              <span className={`hc-key hc-${s.provider}`} aria-hidden="true" />
              {PROVIDER_NAME[s.provider] ?? s.provider}
              <b>{Math.round(value(s.points[s.points.length - 1]!))}%</b>
            </span>
          ))}
        </span>
      </figcaption>
      <div className="hc-plot">
        <svg
          viewBox={`0 0 ${W} ${H}`}
          role="img"
          aria-label={t.chartAria(names)}
          tabIndex={0}
          onPointerMove={(e) => setHoverT(timeAt(e))}
          onPointerLeave={() => setHoverT(null)}
          onFocus={() => hoverT === null && setHoverT(f.t1)}
          onBlur={() => setHoverT(null)}
          onKeyDown={onKey}
        >
          {gaps.map(([a, b]) => {
            const x = xOf(a, f);
            const w = xOf(b, f) - x;
            return (
              <g key={a}>
                <rect className="hc-gap" x={x} y={top} width={w} height={bottom - top} />
                {/* In the 50-75 band: the plot's centre is the 50% gridline, and the grid paints over this. */}
                {w >= 48 && (
                  <text className="hc-gap-label" x={x + w / 2} y={yOf(62.5, f) + 4}>
                    {t.chartNoData}
                  </text>
                )}
              </g>
            );
          })}
          {[0, 25, 50, 75, 100].map((v) => (
            <g key={v}>
              <line className="grid" x1={f.padLeft} x2={W - f.padRight} y1={yOf(v, f)} y2={yOf(v, f)} />
              <text className="axis" x={f.padLeft - 6} y={yOf(v, f) + 4} textAnchor="end">
                {v}%
              </text>
            </g>
          ))}
          {timeTicks(f.t0, f.t1, 7).map((tk) => (
            <text key={tk} className="axis" x={xOf(tk, f)} y={H - 8} textAnchor="middle">
              {tickLabel(tk)}
            </text>
          ))}
          {markers.filter((m) => inRange(m.t)).map((m, i) => (
            <g key={i} className="hc-marker">
              <line x1={xOf(m.t, f)} x2={xOf(m.t, f)} y1={top} y2={bottom} />
              <circle className={`hc-dot hc-${m.provider}`} cx={xOf(m.t, f)} cy={top} r={3.5} />
              <rect x={xOf(m.t, f) - 8} y={top - 8} width={16} height={16} fill="transparent">
                <title>{`${PROVIDER_NAME[m.provider] ?? m.provider} · ${eventTitle({ id: 0, provider: m.provider, eventKey: "", windowType: m.windowType, eventType: m.eventType, visibility: "user", observedAtMs: m.t, confidence: "medium", confirmed: true, evidence: {}, supersededByEventKey: null }, t)} · ${formatDateTime(m.t, lang)}`}</title>
              </rect>
            </g>
          ))}
          {series.map((s) =>
            buildPaths(s.points.filter((p) => inRange(p.t)), value, f).map((d, i) => <path key={`${s.provider}-${i}`} d={d} className={`hc-line hc-${s.provider}`} />),
          )}
          {series.map((s) => {
            const last = s.points[s.points.length - 1]!;
            return inRange(last.t) && <circle key={s.provider} className={`hc-dot hc-${s.provider}`} cx={xOf(last.t, f)} cy={yOf(value(last), f)} r={4} />;
          })}
          {crossT !== null && (
            <g aria-hidden="true">
              <line className="hc-cross" x1={xOf(crossT, f)} x2={xOf(crossT, f)} y1={top} y2={bottom} />
              {hits.map(({ s, p }) => p && <circle key={s.provider} className={`hc-dot hc-${s.provider}`} cx={xOf(p.t, f)} cy={yOf(value(p), f)} r={4} />)}
            </g>
          )}
        </svg>
        {crossT !== null && (
          <div className={`hc-tip${xPct > 65 ? " hc-tip-left" : ""}`} style={{ left: `${xPct}%` }} aria-live="polite">
            <span className="hc-tip-time">{formatDateTime(crossT, lang)}</span>
            {hits.map(({ s, p }) => (
              <span key={s.provider} className="hc-tip-row">
                <span className={`hc-key hc-${s.provider}`} aria-hidden="true" />
                <b>{p ? `${Math.round(value(p))}%` : "—"}</b>
                <span className="muted">{PROVIDER_NAME[s.provider] ?? s.provider}</span>
              </span>
            ))}
          </div>
        )}
      </div>
    </figure>
  );
}

/** The same values without hovering: one row per time step, newest first. */
function HistoryTable({ series, domain, value, tolerance }: { series: Series[]; domain: Domain; value: (p: HistoryPoint) => number; tolerance: number }) {
  const { t, lang } = useApp();
  const times = timeTicks(domain.t0, domain.t1, 24);
  const latest = Math.max(...series.map((s) => s.points[s.points.length - 1]?.t ?? Number.NEGATIVE_INFINITY));
  if (Number.isFinite(latest) && !times.includes(latest)) times.push(latest);
  return (
    <details className="hc-table">
      <summary>{t.chartTable}</summary>
      <div className="hc-table-wrap">
        <table>
          <thead>
            <tr>
              <th>{t.chartTime}</th>
              {series.map((s) => (
                <th key={`${s.provider}-${s.windowType}`}>{`${PROVIDER_NAME[s.provider] ?? s.provider} ${windowName(s.windowType, t)}`}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {[...times].reverse().map((tm) => (
              <tr key={tm}>
                <td>{formatDateTime(tm, lang)}</td>
                {series.map((s) => {
                  const p = pointAt(s.points, tm, tolerance);
                  return <td key={`${s.provider}-${s.windowType}`}>{p ? `${Math.round(value(p))}%` : "—"}</td>;
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </details>
  );
}
