// Forecast card: everything here is a QuotaDock estimate and is labelled as such.

import type { UsageForecast } from "../api/types.ts";
import { useApp } from "../context.ts";
import { burnText, formatDateTime, forecastMode, riskView, spanText } from "../model/present.ts";
import { EstimateBadge, InfoTip, Pill } from "./common.tsx";

function Row({ label, value, tip }: { label: string; value: string; tip?: string }) {
  return (
    <div className="kv">
      <span className="k">
        {label}
        {tip && <InfoTip text={tip} />}
      </span>
      <span className="v">{value}</span>
    </div>
  );
}

function ForecastBlock({ f }: { f: UsageForecast }) {
  const { t, lang } = useApp();
  const title = f.windowType === "weekly" ? t.weeklyShort : t.fiveHourShort;
  const mode = forecastMode(f);
  const risk = riskView(f.risk, t);
  if (mode === "insufficient") {
    return (
      <div className="fc-block">
        <div className="fc-title">{title}</div>
        <div className="fc-empty">{t.notEnough}</div>
        <div className="help">{t.notEnoughHelp(spanText(f.requiredSpanMs, t))}</div>
      </div>
    );
  }
  const burn = burnText(f.burnRatePerHour, f.windowType, t) ?? "—";
  return (
    <div className="fc-block">
      <div className="fc-title">
        {title}
        {f.risk !== "insufficient_data" && <Pill tone={risk.tone}>{risk.label}</Pill>}
      </div>
      <Row label={t.burnRate} value={burn} />
      <Row label={t.exhaustion} value={mode === "no_projection" ? t.noProjection : f.estimatedExhaustionAtMs ? formatDateTime(f.estimatedExhaustionAtMs, lang) : "—"} />
      <Row label={t.resetTime} value={f.serverResetAtMs ? formatDateTime(f.serverResetAtMs, lang) : t.scheduleUnconfirmed} />
      {mode === "unconfirmed" ? (
        <div className="help">{t.scheduleUnconfirmedHelp}</div>
      ) : (
        <>
          {f.marginHours !== null && <Row label={t.margin} value={t.marginHours(f.marginHours.toFixed(1))} />}
          <Row label={t.pressure} value={f.pressure === null ? "—" : f.pressure.toFixed(2)} tip={t.estimateTip} />
          <Row label={t.risk} value={risk.label} tip={t.estimateTip} />
        </>
      )}
    </div>
  );
}

export function ForecastCard({ forecasts, title }: { forecasts: UsageForecast[]; title?: string }) {
  const { t } = useApp();
  const ordered = [...forecasts].sort((a) => (a.windowType === "weekly" ? -1 : 1));
  return (
    <section className="card forecast-card" aria-label={title ? `${t.forecastTitle} · ${title}` : t.forecastTitle}>
      <header className="card-head">
        <h2>{title ? `${t.forecastTitle} · ${title}` : t.forecastTitle}</h2>
        <EstimateBadge />
      </header>
      <div className="fc-grid">
        {ordered.map((f) => (
          <ForecastBlock key={f.windowType} f={f} />
        ))}
      </div>
    </section>
  );
}
