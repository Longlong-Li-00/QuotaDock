// Provider-neutral cards: one card per provider (Codex, Claude) with its usage windows.

import type { DisplayTone, ProviderDashboard, TrayStatusModel, UsageForecast, WindowStatus } from "../api/types.ts";
import { useApp } from "../context.ts";
import type { Dict } from "../i18n/en.ts";
import { agoText, availabilityPill, extraResetPhrase, freshnessView, headPlan, pct, resetPhrase, toneLabel, windowForecastLine, windowReset, windowTone, type Tone } from "../model/present.ts";
import { Meter, Pill } from "./common.tsx";

/** The tone comes from the backend's tray line; the UI applies no thresholds. */
function UsageWindow({ window: w, lineTone, forecast }: { window: WindowStatus; lineTone: DisplayTone | undefined; forecast: UsageForecast | undefined }) {
  const { t, lang, now } = useApp();
  const title = w.windowType === "five_hour" ? t.fiveHour : t.weekly;
  const remaining = w.present ? pct(w.remainingPercent) : null;
  const tone = windowTone(w, lineTone);
  const label = toneLabel(tone, t);
  const note = forecast ? windowForecastLine(w, forecast, now, t, lang) : null;
  const spoken = remaining === null ? `${title} ${t.wNoValue}` : `${title} ${t.remainingWord} ${remaining}%`;
  return (
    <div className="window">
      <div className="window-head">
        <span className="window-title">{title}</span>
        {label && <span className={`tone-${tone}`}>{label}</span>}
      </div>
      <div className={`big tone-${tone}`}>
        {remaining === null ? (
          <span className="big-num">—</span>
        ) : (
          <>
            <span className="big-num">{remaining}</span>
            <span className="big-unit">%</span>
            <span className="big-word">{t.remainingWord}</span>
          </>
        )}
      </div>
      <Meter value={remaining} tone={tone} label={label ? `${spoken} ${label}` : spoken} />
      <div className="reset">{resetPhrase(windowReset(w), "overview", now, t, lang)}</div>
      {note && <div className="forecast-note">{note}</div>}
    </div>
  );
}

/** The card's own banner for its health state, or null. Paused has one banner above the cards instead. */
export function healthBanner(provider: ProviderDashboard, t: Dict, now: number): { title: string; body?: string; tone: Tone } | null {
  const claude = provider.provider === "claude";
  const src = provider.source;
  const ago = src?.lastUpdateMs ? agoText(src.lastUpdateMs, now, t) : "—";
  const map: Record<string, { title: string; body?: string; tone: Tone } | undefined> = claude
    ? {
        not_found: { title: t.claudeNotFound, body: t.claudeNotFoundHelp, tone: "neutral" },
        not_authenticated: { title: t.claudeNotAuthenticated, body: t.claudeNotAuthenticatedHelp, tone: "warning" },
        api_billing: { title: t.claudeApiBilling, body: t.claudeApiBillingHelp, tone: "warning" },
        not_configured: { title: t.claudeNotConfigured, body: t.claudeNotConfiguredHelp, tone: "neutral" },
        waiting: { title: t.claudeWaiting, body: t.claudeWaitingHelp, tone: "neutral" },
        stale: { title: t.claudeStale, body: t.claudeStaleHelp(ago), tone: "neutral" },
        source_error: { title: t.claudeSourceError, body: t.claudeSourceErrorHelp, tone: "warning" },
        needs_verification: { title: t.claudeNeedsVerification, body: t.claudeNeedsVerificationHelp, tone: "warning" },
      }
    : {
        connecting: { title: t.connecting, tone: "neutral" },
        not_found: { title: t.notFound, body: t.notFoundHelp, tone: "critical" },
        not_authenticated: { title: t.notAuthenticated, body: t.notAuthenticatedHelp, tone: "critical" },
        unsupported_version: { title: t.unsupportedVersion, body: t.unsupportedVersionHelp, tone: "warning" },
        reconnecting: { title: t.reconnecting, body: t.reconnectingHelp, tone: "warning" },
      };
  return map[provider.health.state] ?? null;
}

function HealthBanner({ provider }: { provider: ProviderDashboard }) {
  const { t, now } = useApp();
  const claude = provider.provider === "claude";
  const src = provider.source;
  const b = healthBanner(provider, t, now);
  // Active refresh limited by the server: the card keeps its last data and says so (no error state).
  const limited = claude && src?.activeEnabled && (src.activeSuspended || (src.activeNextAllowedMs !== null && src.activeNextAllowedMs > now));
  if (!b && !limited) return null;
  return (
    <>
      {b && (
        <div className={`banner banner-${b.tone}`} role="status">
          <strong>{b.title}</strong>
          {b.body && <div>{b.body}</div>}
        </div>
      )}
      {limited && (
        <div className="banner banner-neutral" role="status">
          {src?.activeSuspended ? t.activeSuspended : t.activeRateLimited}
        </div>
      )}
    </>
  );
}

/** The provider's own age. Codex keeps its freshness wording and tones; Claude says when it is not live. */
function ProviderAge({ provider, tray }: { provider: ProviderDashboard; tray: TrayStatusModel }) {
  const { t, now } = useApp();
  if (provider.provider !== "claude") {
    if (provider.usage.capturedAtMs === null) return null;
    const fresh = freshnessView(tray, now, t);
    return <span className={`fresh fresh-${fresh.tone}`}>{fresh.text}</span>;
  }
  const s = provider.source;
  if (!s || s.lastUpdateMs === null) return null;
  const ago = agoText(s.lastUpdateMs, now, t);
  return <span className="fresh">{s.live ? ago : `${ago} · ${t.notLive}`}</span>;
}

/** Quiet rows. These times are as the source reports them, not confirmed: a clock, never a countdown. */
function ExtraLimits({ provider }: { provider: ProviderDashboard }) {
  const { t, lang, now } = useApp();
  if (provider.extraLimits.length === 0) return null;
  return (
    <div className="extra-limits">
      {provider.extraLimits.length > 1 && <div className="extra-title">{t.extraLimitsTitle}</div>}
      {provider.extraLimits.map((x) => (
        <div key={x.key} className="kv">
          <span className="k">{t.extraLimitWeekly(x.label)}</span>
          <span className="v">
            {x.current ? `${Math.round(x.remainingPercent)}% ${t.remainingWord}` : `${Math.round(x.remainingPercent)}% · ${t.previousPeriod}`}
            {x.resetsAtMs !== null && x.resetsAtMs > now && x.current && ` · ${extraResetPhrase(x.resetsAtMs, x.resetPrecisionMs, now, t, lang)}`}
          </span>
        </div>
      ))}
    </div>
  );
}

export function ProviderCard({ provider, tray }: { provider: ProviderDashboard; tray: TrayStatusModel }) {
  const { t } = useApp();
  const u = provider.usage;
  const line = tray.providers.find((p) => p.provider === provider.provider);
  const hasData = u.capturedAtMs !== null;
  const pill = hasData ? availabilityPill(provider.provider, u.ordinaryUsageAllowed, tray.availabilityUnknownPersistent, t) : null;
  const weeklyForecast = provider.forecasts.find((f) => f.windowType === "weekly");
  return (
    <section className="card provider-card" aria-label={provider.displayName}>
      <header className="provider-head">
        <div className="provider-title">
          <span className="provider-name">{provider.displayName}</span>
          {provider.planType && <span> · {headPlan(provider.provider, provider.displayName, provider.planType, t)}</span>}
          {provider.compatibilityMode && <span title={t.compatibilityTip}> · {t.compatibilityMode}</span>}
        </div>
        <div className="provider-status">
          {pill && <Pill tone={pill.tone}>{pill.label}</Pill>}
          <ProviderAge provider={provider} tray={tray} />
        </div>
      </header>
      <HealthBanner provider={provider} />
      {hasData && (
        <div className="windows">
          {u.windows.map((w) => (
            <UsageWindow
              key={w.windowType}
              window={w}
              lineTone={w.windowType === "five_hour" ? line?.fiveHourTone : line?.weeklyTone}
              forecast={w.windowType === "weekly" ? weeklyForecast : undefined}
            />
          ))}
        </div>
      )}
      {hasData && <ExtraLimits provider={provider} />}
    </section>
  );
}
