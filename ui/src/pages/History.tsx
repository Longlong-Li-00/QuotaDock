import { useEffect, useState } from "react";
import { backend } from "../api/backend.ts";
import type { HistoryData, HistoryRange, ProviderId } from "../api/types.ts";
import { useApp } from "../context.ts";
import { HistoryChart } from "../components/HistoryChart.tsx";
import { burnText } from "../model/present.ts";
import { EstimateBadge } from "../components/common.tsx";
import { HISTORY_RANGES, loadHistoryRange, saveHistoryRange } from "../model/prefs.ts";

type ProviderFilter = "all" | ProviderId;

export function History() {
  const { t, dashboard } = useApp();
  // Opens on 12 hours, or on the range chosen last time.
  const [range, setRangeState] = useState<HistoryRange>(loadHistoryRange);
  const setRange = (r: HistoryRange) => {
    setRangeState(r);
    saveHistoryRange(r);
  };
  const [mode, setMode] = useState<"remaining" | "used">("remaining");
  const [provider, setProvider] = useState<ProviderFilter>("all");
  const [data, setData] = useState<HistoryData | null>(null);
  const [loading, setLoading] = useState(true);
  const stamp = dashboard?.tray.updatedAtMs ?? 0;

  useEffect(() => {
    let alive = true;
    setLoading(true);
    backend
      .getUsageHistory(range)
      .then((d) => alive && setData(d))
      .finally(() => alive && setLoading(false));
    return () => {
      alive = false;
    };
  }, [range, stamp]);

  const providers = dashboard?.providers ?? [];
  const showSelector = providers.length > 1;
  const visible = data && {
    ...data,
    series: data.series.filter((s) => provider === "all" || s.provider === provider),
    markers: data.markers.filter((m) => provider === "all" || m.provider === provider),
  };
  const empty = !visible || visible.series.every((s) => s.points.length === 0);
  const shown = providers.filter((p) => provider === "all" || p.provider === provider);
  return (
    <div className="history">
      <div className="toolbar">
        <div className="seg" role="tablist" aria-label={t.historyTitle}>
          {HISTORY_RANGES.map((r) => (
            <button key={r} type="button" role="tab" aria-selected={range === r} className={range === r ? "on" : ""} onClick={() => setRange(r)}>
              {t[`range_${r}`]}
            </button>
          ))}
        </div>
        {showSelector && (
          <div className="seg" role="group" aria-label={t.secProviders}>
            {(["all", ...providers.map((p) => p.provider as ProviderId)] as ProviderFilter[]).map((p) => (
              <button key={p} type="button" aria-pressed={provider === p} className={provider === p ? "on" : ""} onClick={() => setProvider(p)}>
                {p === "all" ? t.providerAll : (providers.find((x) => x.provider === p)?.displayName ?? p)}
              </button>
            ))}
          </div>
        )}
        <div className="seg" role="group">
          <button type="button" aria-pressed={mode === "remaining"} className={mode === "remaining" ? "on" : ""} onClick={() => setMode("remaining")}>
            {t.showRemaining}
          </button>
          <button type="button" aria-pressed={mode === "used"} className={mode === "used" ? "on" : ""} onClick={() => setMode("used")}>
            {t.showUsed}
          </button>
        </div>
      </div>
      <section className="card">
        {loading && !data ? <div className="muted">…</div> : empty ? <div className="muted">{t.noHistory}</div> : <HistoryChart data={visible!} mode={mode} />}
      </section>
      {shown.map((p) => (
        <section key={p.provider} className="card">
          <header className="card-head">
            <h2>{showSelector ? `${t.burnRate} · ${p.displayName}` : t.burnRate}</h2>
            <EstimateBadge />
          </header>
          <div className="burn-grid">
            {p.forecasts.length === 0 && <div className="help">{t.notEnough}</div>}
            {p.forecasts.map((f) => (
              <div key={f.windowType} className="burn-col">
                <div className="fc-title">{f.windowType === "weekly" ? t.weeklyShort : t.fiveHourShort}</div>
                {f.burn.status === "insufficient_data" ? (
                  <div className="help">{t.notEnough}</div>
                ) : (
                  <>
                    <div className="kv">
                      <span className="k">{t.burnCurrent}</span>
                      <span className="v">{burnText(f.burn.recent6hRate, f.windowType, t) ?? "—"}</span>
                    </div>
                    <div className="kv">
                      <span className="k">{t.burn24h}</span>
                      <span className="v">{burnText(f.burn.recent24hRate, f.windowType, t) ?? "—"}</span>
                    </div>
                    <div className="kv">
                      <span className="k">{t.burnCycle}</span>
                      <span className="v">{burnText(f.burn.currentCycleRate, f.windowType, t) ?? "—"}</span>
                    </div>
                  </>
                )}
              </div>
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
