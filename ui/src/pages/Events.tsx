import { useCallback, useEffect, useState } from "react";
import { backend } from "../api/backend.ts";
import type { EventView } from "../api/types.ts";
import { useApp } from "../context.ts";
import { EventList } from "../components/EventList.tsx";
import { Toggle } from "../components/common.tsx";

const PAGE = 50;

export function Events() {
  const { t, settings, saveSettings, dashboard } = useApp();
  const showDiag = settings?.settings.showDiagnostics ?? false;
  const [events, setEvents] = useState<EventView[]>([]);
  const [more, setMore] = useState(false);
  const stamp = dashboard?.tray.updatedAtMs ?? 0;

  const load = useCallback(
    async (beforeMs: number | null) => {
      const page = await backend.getQuotaEvents(showDiag, beforeMs, PAGE);
      setEvents((prev) => (beforeMs === null ? page : [...prev, ...page]));
      setMore(page.length === PAGE);
    },
    [showDiag],
  );

  useEffect(() => {
    void load(null);
  }, [load, stamp]);

  return (
    <div className="events-page">
      <div className="toolbar">
        <h2 className="toolbar-title">{showDiag ? `${t.userEvents} + ${t.diagnostics}` : t.userEvents}</h2>
        <div className="toolbar-toggle">
          <Toggle label={t.showDiagnostics} checked={showDiag} onChange={(v) => settings && saveSettings({ ...settings.settings, showDiagnostics: v })} />
        </div>
      </div>
      <section className="card">
        {events.length === 0 ? <div className="muted">{t.noEvents}</div> : <EventList events={events} />}
        {more && (
          <button type="button" className="btn" onClick={() => load(events[events.length - 1]?.observedAtMs ?? null)}>
            {t.loadMore}
          </button>
        )}
      </section>
    </div>
  );
}
