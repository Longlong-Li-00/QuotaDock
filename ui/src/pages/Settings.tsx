import { useEffect, useState } from "react";
import { version } from "../../package.json";
import { backend } from "../api/backend.ts";
import type { AppSettings, ClaudeNotificationSettings, ClaudeSettings, CodexSettings, DesktopWidgetSettings, LanguagePref, NotificationSettings, ThemePref, WidgetMode } from "../api/types.ts";
import { useApp } from "../context.ts";
import { Pill, Section, Toggle } from "../components/common.tsx";
import { ClaudeIntegrationPanel, ClaudeOffDialog } from "../components/ClaudeIntegration.tsx";
import { ForecastCard } from "../components/ForecastCard.tsx";
import { agoText } from "../model/present.ts";
import { enabledReminderCount, withApp, withClaude, withClaudeNotifications, withCodex, withNotifications, withWidget } from "../model/settings.ts";
import { lookup } from "../i18n/index.ts";

export function Settings() {
  const { t, settings, saveSettings, dashboard, reload, now } = useApp();
  const [exported, setExported] = useState<string | null>(null);
  const [claudeOff, setClaudeOff] = useState<"idle" | "ask" | "busy">("idle");
  const [claudeOffError, setClaudeOffError] = useState<string | null>(null);
  if (!settings) return null;
  const s = settings.settings;
  const set = (patch: Partial<AppSettings>) => saveSettings(withApp(s, patch));
  const setN = (patch: Partial<NotificationSettings>) => saveSettings(withNotifications(s, patch));
  const setC = (patch: Partial<ClaudeSettings>) => saveSettings(withClaude(s, patch));
  const setCN = (patch: Partial<ClaudeNotificationSettings>) => saveSettings(withClaudeNotifications(s, patch));
  const setCodex = (patch: Partial<CodexSettings>) => saveSettings(withCodex(s, patch));
  const setW = (patch: Partial<DesktopWidgetSettings>) => saveSettings(withWidget(s, patch));
  const w = s.desktopWidget;
  // §46: turning Claude off must not leave QuotaDock's status-line bridge in Claude Code's settings.
  const toggleClaude = async (on: boolean) => {
    if (on) return setC({ enabled: true });
    const view = await backend.getClaudeIntegration().catch(() => null);
    if (view?.enabled) setClaudeOff("ask");
    else setC({ enabled: false });
  };
  const confirmClaudeOff = async () => {
    setClaudeOff("busy");
    setClaudeOffError(null);
    try {
      await backend.disableClaudeIntegration();
      setClaudeOff("idle");
      setC({ enabled: false });
      reload();
    } catch (e) {
      setClaudeOffError(String(e));
      setClaudeOff("ask");
    }
  };
  const n = s.notifications;
  const codex = dashboard?.providers.find((p) => p.provider === "codex");
  const claude = dashboard?.providers.find((p) => p.provider === "claude");
  const claudeState = claude?.source?.state;
  return (
    <div className="settings">
      <Section title={t.secGeneral}>
        <Toggle label={t.launchAtStartup} checked={s.launchAtStartup} onChange={(v) => set({ launchAtStartup: v })} />
        <label className="field">
          <span>{t.language}</span>
          <select value={s.language} onChange={(e) => set({ language: e.target.value as LanguagePref })}>
            <option value="system">{t.langSystem}</option>
            <option value="zh-CN">中文</option>
            <option value="en">English</option>
          </select>
        </label>
        <label className="field">
          <span>{t.theme}</span>
          <select value={s.theme} onChange={(e) => set({ theme: e.target.value as ThemePref })}>
            <option value="system">{t.themeSystem}</option>
            <option value="light">{t.themeLight}</option>
            <option value="dark">{t.themeDark}</option>
          </select>
        </label>
      </Section>

      <Section title={t.secWidget}>
        <Toggle label={t.widgetShow} checked={w.enabled} onChange={(v) => setW({ enabled: v })} />
        <OpacityField label={t.widgetOpacity} help={t.widgetOpacityHelp} value={w.opacity} onCommit={(v) => setW({ opacity: v })} />
        <div className="help section-note">{t.widgetHowTo}</div>
      </Section>

      <Section title={t.secProviders}>
        <div className="provider-setting">
          <div className="ps-head">
            <strong>Codex</strong>
            {s.codex.enabled && codex && codex.health.state !== "ok" && <Pill tone="warning">{lookup(t, `state_${codex.health.state}`, codex.health.state)}</Pill>}
          </div>
          <Toggle label={t.providerEnabled} help={t.codexEnabledHelp} checked={s.codex.enabled} onChange={(v) => void setCodex({ enabled: v })} />
          {s.codex.enabled && <Toggle label={t.showFiveHour} help={t.showFiveHourHelp} checked={s.codex.showFiveHour} onChange={(v) => void setCodex({ showFiveHour: v })} />}
        </div>
        <div className="provider-setting">
          <div className="ps-head">
            <strong>Claude</strong>
            {s.claude.enabled && claudeState && claudeState !== "live" && <Pill tone="neutral">{lookup(t, `state_${claudeState}`, claudeState)}</Pill>}
          </div>
          <Toggle label={t.providerEnabled} help={t.claudeEnabledHelp} checked={s.claude.enabled} onChange={(v) => void toggleClaude(v)} />
          {s.claude.enabled && (
            <>
              <Toggle label={t.showFiveHour} help={t.showFiveHourHelpClaude} checked={s.claude.showFiveHour} onChange={(v) => setC({ showFiveHour: v })} />
              <Toggle label={t.activeRefresh} help={`${t.activeRefreshHelp} ${t.activeRefreshCadence}`} checked={s.claude.activeRefresh ?? true} onChange={(v) => setC({ activeRefresh: v })} />
            </>
          )}
          <ClaudeIntegrationPanel key={s.claude.enabled ? "on" : "off"} onChanged={reload} monitoringOn={s.claude.enabled} />
          {claudeOff !== "idle" && (
            <ClaudeOffDialog onCancel={() => setClaudeOff("idle")} onConfirm={() => void confirmClaudeOff()} busy={claudeOff === "busy"} error={claudeOffError} />
          )}
        </div>
      </Section>

      {(s.codex.enabled || s.claude.enabled) && (
        <Section title={t.secNotifications}>
          <details className="disclosure">
            <summary>
              <span>{t.nEnabledCount(enabledReminderCount(s))}</span>
              <span className="disclosure-action">{t.nAdjust}</span>
            </summary>
            {s.codex.enabled && (
              <>
                <div className="group-title">{t.nGroupCodex}</div>
                <Toggle label={t.nWeeklyQuota} help={t.nWeeklyQuotaHelp} checked={n.weeklyQuota} onChange={(v) => setN({ weeklyQuota: v })} />
                {s.codex.showFiveHour && <Toggle label={t.nFiveHourQuota} help={t.nFiveHourQuotaHelp} checked={n.fiveHourQuota} onChange={(v) => setN({ fiveHourQuota: v })} />}
                <Toggle label={t.nForecast} help={t.nForecastHelp} checked={n.forecastWeekly} onChange={(v) => setN({ forecastWeekly: v })} />
                {s.codex.showFiveHour && <Toggle label={t.nForecastFiveHour} checked={n.forecastFiveHour} onChange={(v) => setN({ forecastFiveHour: v })} />}
                <Toggle label={t.nAvailability} help={t.nAvailabilityHelp} checked={n.availabilityRestored} onChange={(v) => setN({ availabilityRestored: v })} />
                <Toggle label={t.nQuotaRefresh} help={t.nQuotaRefreshHelp} checked={n.quotaRefresh} onChange={(v) => setN({ quotaRefresh: v })} />
                <Toggle label={t.nResetCredit} help={t.nResetCreditHelp} checked={n.resetCredit} onChange={(v) => setN({ resetCredit: v })} />
                <Toggle label={t.nAllowance} help={s.codex.showFiveHour ? t.nAllowanceHelp : t.nAllowanceHelpWeekly} checked={n.allowanceRefresh} onChange={(v) => setN({ allowanceRefresh: v })} />
                <Toggle label={t.nProviderErrors} help={t.nProviderErrorsHelp} checked={n.providerErrors} onChange={(v) => setN({ providerErrors: v })} />
              </>
            )}
            {s.claude.enabled && (
              <>
                <div className="group-title">{t.nGroupClaude}</div>
                <Toggle label={t.nClaudeWeekly} help={t.nWeeklyQuotaHelp} checked={n.claude.weeklyQuota} onChange={(v) => setCN({ weeklyQuota: v })} />
                {s.claude.showFiveHour && <Toggle label={t.nClaudeFiveHour} help={t.nFiveHourQuotaHelp} checked={n.claude.fiveHourQuota} onChange={(v) => setCN({ fiveHourQuota: v })} />}
                <Toggle label={t.nClaudeForecast} help={t.nClaudeForecastHelp} checked={n.claude.forecastWeekly} onChange={(v) => setCN({ forecastWeekly: v })} />
                <Toggle label={t.nClaudeAllowance} help={s.claude.showFiveHour ? t.nAllowanceHelp : t.nAllowanceHelpWeekly} checked={n.claude.allowanceRefresh} onChange={(v) => setCN({ allowanceRefresh: v })} />
              </>
            )}
          </details>
        </Section>
      )}

      <details className="card section disclosure disclosure-section">
        <summary>
          <h2>{t.secAdvanced}</h2>
        </summary>
        <Toggle label={t.closeToTray} help={t.closeToTrayHelp} checked={s.closeToTray} onChange={(v) => set({ closeToTray: v })} />

        <div className="group-title">{t.secWidget}</div>
        <div className="help">{t.widgetShowHelp}</div>
        <Toggle label={t.widgetOnTop} checked={w.alwaysOnTop} onChange={(v) => setW({ alwaysOnTop: v })} />
        <label className="field">
          <span>{t.widgetLayout}</span>
          <select value={w.mode} onChange={(e) => setW({ mode: e.target.value as WidgetMode })}>
            <option value="compact">{t.widgetCompact}</option>
            <option value="detailed">{t.widgetDetailed}</option>
          </select>
        </label>
        <Toggle label={t.widgetRemember} checked={w.rememberPosition} onChange={(v) => setW({ rememberPosition: v })} />
        <Toggle label={t.widgetLock} help={t.widgetLockHelp} checked={w.locked} onChange={(v) => setW({ locked: v })} />

        <div className="group-title">{t.secDiagnostics}</div>
        <Toggle label={t.showDiagnosticEvents} checked={s.showDiagnostics} onChange={(v) => set({ showDiagnostics: v })} />
        {s.codex.enabled && (
          <>
            <div className="kv">
              <span className="k">{t.backgroundRefresh}</span>
              <span className="v">{t.every5min}</span>
            </div>
            <div className="kv">
              <span className="k">{t.foregroundRefresh}</span>
              <span className="v">{t.every1min}</span>
            </div>
            <div className="help">{t.fixedIntervalsNote}</div>
          </>
        )}
        {s.claude.enabled && <div className="help">{t.claudeCadence}</div>}
        {s.codex.enabled && (
          <div className="kv">
            <span className="k">{t.sourceLabel} · Codex</span>
            <span className="v">{t.sourceCodexAppServer}</span>
          </div>
        )}
        {s.claude.enabled && claude?.source?.lastUpdateMs != null && (
          <div className="kv">
            <span className="k">{t.sourceLabel} · Claude</span>
            <span className="v">
              {claude.source.lastSource === "active_cli" ? t.sourceActive : t.sourceStatusLine} · {t.lastUpdatedAgo(agoText(claude.source.lastUpdateMs, now, t))}
            </span>
          </div>
        )}
        {codex && codex.forecasts.length > 0 && <ForecastCard forecasts={codex.forecasts} title="Codex" />}
        {claude && claude.forecasts.length > 0 && <ForecastCard forecasts={claude.forecasts} title="Claude" />}
        <div className="actions">
          <button type="button" className="btn" onClick={() => void backend.openDataFolder()}>
            {t.openDataFolder}
          </button>
          <button
            type="button"
            className="btn"
            onClick={async () => {
              try {
                setExported(t.exported(await backend.exportDiagnostics()));
              } catch (e) {
                setExported(t.exportFailed(String(e)));
              }
            }}
          >
            {t.exportDiagnostics}
          </button>
        </div>
        {exported && <div className="help">{exported}</div>}
        <div className="help">{t.privacyNote}</div>
        <div className="kv">
          <span className="k">{t.appVersion}</span>
          <span className="v">{version}</span>
        </div>

        <div className="quit">
          <div className="help">{t.quitHelp}</div>
          <button type="button" className="btn btn-danger" onClick={() => void backend.quitApp()}>
            {t.quitApp}
          </button>
        </div>
      </details>
    </div>
  );
}

/** 50-100 %, saved when the slider is released (not on every step). */
function OpacityField({ label, help, value, onCommit }: { label: string; help: string; value: number; onCommit: (v: number) => void }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  const commit = () => {
    if (v !== value) onCommit(v);
  };
  return (
    <label className="field">
      <span className="field-text">
        <span>{label}</span>
        <span className="help">{help}</span>
      </span>
      <span className="range">
        <input type="range" min={50} max={100} step={5} value={v} onChange={(e) => setV(Number(e.target.value))} onPointerUp={commit} onKeyUp={commit} onBlur={commit} />
        <span className="range-value">{v}%</span>
      </span>
    </label>
  );
}
