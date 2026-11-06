import { useApp } from "../context.ts";
import { healthBanner, ProviderCard } from "../components/ProviderCard.tsx";
import { topBanner } from "../model/present.ts";

export function Overview() {
  const { dashboard, t, now } = useApp();
  if (!dashboard) return <div className="card muted">{t.connecting}</div>;
  if (dashboard.providers.length === 0) return <div className="card muted">{t.noProvidersEnabled}</div>;
  // A card with a banner of its own says why; the line above repeats only a limit.
  const covered = new Set(dashboard.providers.filter((p) => healthBanner(p, t, now) !== null).map((p) => p.provider));
  const banner = topBanner(dashboard.tray, t, covered);
  return (
    <div className="overview">
      {dashboard.tray.paused && (
        <div className="banner banner-neutral" role="status">
          <strong>{t.monitoringPaused}</strong>
          <div>{t.pausedHelp}</div>
        </div>
      )}
      {banner && (
        <div className={`status-line status-${banner.tone}`} role="status">
          {banner.text}
        </div>
      )}
      {dashboard.providers.map((p) => (
        <ProviderCard key={p.provider} provider={p} tray={dashboard.tray} />
      ))}
    </div>
  );
}
