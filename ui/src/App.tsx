import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { backend, inTauri } from "./api/backend.ts";
import type { AppSettings, DashboardDto, RefreshDecision, SettingsDto } from "./api/types.ts";
import { AppContext, type AppCtx } from "./context.ts";
import { dictFor } from "./i18n/index.ts";
import { settingsQueue } from "./model/settings.ts";
import { Overview } from "./pages/Overview.tsx";
import { History } from "./pages/History.tsx";
import { Events } from "./pages/Events.tsx";
import { Settings } from "./pages/Settings.tsx";
import { CloseHintDialog } from "./components/CloseHintDialog.tsx";

type Page = "overview" | "history" | "events" | "settings";

declare global {
  interface Window {
    __quotadock?: { refresh: () => void; reloadSettings: () => void; showCloseHint: () => void; hideCloseHint: () => void; setActive: (active: boolean) => void };
  }
}

/** Local re-read interval while the window is shown. Reads SQLite only, never the backend. */
const LOCAL_REFRESH_MS = 5_000;

export function App() {
  const [page, setPage] = useState<Page>("overview");
  const [dashboard, setDashboard] = useState<DashboardDto | null>(null);
  const [settings, setSettings] = useState<SettingsDto | null>(null);
  const [now, setNow] = useState(Date.now());
  const [closeHint, setCloseHint] = useState(false);
  // Set by the backend when the window is shown/hidden/minimized (WebView2 keeps running in the tray).
  const [hookActive, setHookActive] = useState<boolean | null>(null);
  const [refresh, setRefresh] = useState<"busy" | RefreshDecision | null>(null);
  const refreshNoteTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [saves] = useState(() => settingsQueue<SettingsDto>({ send: (next) => backend.updateSettings(next), read: () => backend.getSettings(), apply: setSettings }));

  const reload = useCallback(async () => {
    try {
      setDashboard(await backend.getDashboard());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void saves.read();
    window.__quotadock = {
      refresh: () => void reload(),
      // Settings changed outside this page (the desktop widget's own menu).
      reloadSettings: () => void saves.read(),
      showCloseHint: () => setCloseHint(true),
      // The backend hid the window some other way (second close, tray): drop the hint unacknowledged.
      hideCloseHint: () => setCloseHint(false),
      setActive: (a) => setHookActive(a),
    };
  }, [reload, saves]);
  useEffect(() => () => clearTimeout(refreshNoteTimer.current), []);

  // Until the first hook call (e.g. an autostart launch hidden in the tray), the backend's
  // foreground flag decides. While hidden, no timers run; polls still nudge `refresh`.
  const active = hookActive ?? dashboard?.polling?.foreground ?? true;
  useEffect(() => {
    if (!active) return;
    void reload();
    setNow(Date.now());
    const local = setInterval(() => void reload(), LOCAL_REFRESH_MS);
    const clock = setInterval(() => setNow(Date.now()), 1_000);
    return () => {
      clearInterval(local);
      clearInterval(clock);
    };
  }, [active, reload]);

  const lang = settings?.effectiveLanguage ?? dashboard?.effectiveLanguage ?? "en";
  const t = dictFor(lang);
  const theme = settings?.settings.theme ?? "system";

  useEffect(() => {
    document.documentElement.lang = lang;
    if (theme === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = theme;
  }, [lang, theme]);

  const saveSettings = useCallback(
    async (next: AppSettings) => {
      // At once, so the switch moves and the next click builds on this copy, not the one before it.
      setSettings((current) => current && { ...current, settings: next });
      await saves.save(next);
      void reload();
    },
    [reload, saves],
  );

  const ctx: AppCtx = useMemo(() => ({ t, lang, now, dashboard, settings, reload: () => void reload(), saveSettings }), [t, lang, now, dashboard, settings, reload, saveSettings]);

  const doRefresh = async () => {
    clearTimeout(refreshNoteTimer.current); // an earlier click's timer must not end this click's note
    setRefresh("busy");
    const d = await backend.refreshUsage();
    setRefresh(d);
    refreshNoteTimer.current = setTimeout(() => setRefresh(null), 2_500);
    void reload();
  };
  // Pausing lives in the tray; the window only offers the way back.
  const resume = async () => {
    await backend.resumeMonitoring();
    void reload();
  };

  const noProviders = dashboard?.providers.length === 0;
  const pages: { id: Page; label: string }[] = [
    { id: "overview", label: t.navOverview },
    { id: "history", label: t.navHistory },
    { id: "events", label: t.navEvents },
    { id: "settings", label: t.navSettings },
  ];
  const refreshNote =
    refresh === "busy" || refresh === "started" ? t.refreshing : refresh === "throttled" || refresh === "coalesced" ? t.refreshThrottled : refresh === "paused" ? t.refreshPaused : null;

  return (
    <AppContext.Provider value={ctx}>
      <div className="shell">
        <nav className="sidebar" aria-label="QuotaDock">
          {pages.map((p) => (
            <button key={p.id} type="button" className={`nav ${page === p.id ? "active" : ""}`} aria-current={page === p.id ? "page" : undefined} onClick={() => setPage(p.id)}>
              {p.label}
            </button>
          ))}
          {!inTauri && <div className="preview-note">browser preview (mock data)</div>}
        </nav>
        <main className="main">
          <header className="topbar">
            <h1>{pages.find((p) => p.id === page)?.label}</h1>
            <div className="topbar-right">
              {page === "overview" && (
                <>
                  <span className="fresh" role="status">
                    {!dashboard?.paused && refreshNote}
                  </span>
                  <button type="button" className="icon-btn" aria-label={t.refresh} title={t.refresh} onClick={doRefresh} disabled={refresh === "busy" || dashboard?.paused || noProviders}>
                    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                      <path d="M13.5 8A5.5 5.5 0 1 1 11.9 4.1" />
                      <path d="M12.5 1.5v3h-3" />
                    </svg>
                  </button>
                </>
              )}
              {dashboard?.paused && (
                <>
                  <span className="fresh fresh-warning">{t.monitoringPaused}</span>
                  <button type="button" className="btn" onClick={resume}>
                    {t.resume}
                  </button>
                </>
              )}
            </div>
          </header>
          {error && <div className="banner banner-critical">{error}</div>}
          <div className="content">
            {page === "overview" && <Overview />}
            {page === "history" && <History />}
            {page === "events" && <Events />}
            {page === "settings" && <Settings />}
          </div>
        </main>
      </div>
      {closeHint && (
        <CloseHintDialog
          onDone={async (dontShowAgain) => {
            setCloseHint(false);
            await backend.acknowledgeCloseHint(dontShowAgain);
            void saves.read();
          }}
        />
      )}
    </AppContext.Provider>
  );
}
