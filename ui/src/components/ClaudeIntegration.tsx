// Opt-in Claude Code status-line bridge. Nothing is written until the user reviews the exact
// change and clicks Enable; disabling restores the previous statusLine.

import { useCallback, useEffect, useRef, useState } from "react";
import { backend } from "../api/backend.ts";
import type { ClaudeIntegrationView, DisableOutcome } from "../api/types.ts";
import { useApp } from "../context.ts";
import { Pill } from "./common.tsx";

function Json({ value, none }: { value: unknown; none: string }) {
  return <pre className="json">{value ? JSON.stringify(value, null, 2) : none}</pre>;
}

function ReviewDialog({ view, onCancel, onEnable, busy, error }: { view: ClaudeIntegrationView; onCancel: () => void; onEnable: () => void; busy: boolean; error: string | null }) {
  const { t } = useApp();
  const plan = view.plan!;
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => cancel.current?.focus(), []);
  return (
    <div className="modal-backdrop">
      <div className="modal modal-wide" role="dialog" aria-modal="true" aria-labelledby="claude-review-title">
        <h2 id="claude-review-title">{t.enableIntegration}</h2>
        <p>{t.integrationExplain}</p>
        <dl className="kvs">
          <dt>{t.integrationFile}</dt>
          <dd className="mono">{plan.settingsPath}</dd>
          <dt>{t.integrationCurrent}</dt>
          <dd>
            <Json value={plan.currentStatusLine} none={t.integrationNone} />
          </dd>
          <dt>{t.integrationNew}</dt>
          <dd>
            <Json value={plan.newStatusLine} none={t.integrationNone} />
          </dd>
          {plan.settingsExists && (
            <>
              <dt>{t.integrationBackup}</dt>
              <dd className="mono">{plan.backupPath}</dd>
            </>
          )}
        </dl>
        <p className="help">{plan.chainsExisting ? t.integrationChained : t.integrationNoExisting}</p>
        <p className="help">{t.integrationPrivacy}</p>
        {error && <div className="banner banner-warning">{error}</div>}
        <div className="actions">
          <button ref={cancel} type="button" className="btn" onClick={onCancel} disabled={busy}>
            {t.cancel}
          </button>
          <button type="button" className="btn btn-primary" onClick={onEnable} disabled={busy}>
            {t.integrationEnable}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Turning Claude monitoring off while the bridge is installed also restores Claude Code's settings (§46). */
export function ClaudeOffDialog({ onCancel, onConfirm, busy, error }: { onCancel: () => void; onConfirm: () => void; busy: boolean; error: string | null }) {
  const { t } = useApp();
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => cancel.current?.focus(), []);
  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="claude-off-title">
        <h2 id="claude-off-title">{t.claudeOffTitle}</h2>
        <p>{t.claudeOffBody}</p>
        {error && <div className="banner banner-warning">{error}</div>}
        <div className="actions">
          <button ref={cancel} type="button" className="btn" onClick={onCancel} disabled={busy}>
            {t.cancel}
          </button>
          <button type="button" className="btn btn-primary" onClick={onConfirm} disabled={busy}>
            {t.claudeOffConfirm}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Shown while Claude monitoring is on, and also while it is off but the bridge is still installed. */
export function ClaudeIntegrationPanel({ onChanged, monitoringOn }: { onChanged: () => void; monitoringOn: boolean }) {
  const { t } = useApp();
  const [view, setView] = useState<ClaudeIntegrationView | null>(null);
  const [review, setReview] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await backend.getClaudeIntegration());
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const enable = async () => {
    if (!view?.plan) return;
    setBusy(true);
    setError(null);
    try {
      setView(await backend.enableClaudeIntegration(view.plan.plannedAtMs, view.plan.planId));
      setReview(false);
      setMessage(null);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const disable = async () => {
    setBusy(true);
    try {
      const outcome: DisableOutcome = await backend.disableClaudeIntegration();
      setMessage(outcome === "changed_by_user" ? t.integrationChangedByUser : t.integrationRestored);
      await load();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!view) return null;
  const on = view.enabled !== null;
  if (!monitoringOn && !on) return null;
  return (
    <div className="integration">
      <div className="kv">
        <span className="k">{t.integrationTitle}</span>
        <span className="v">
          <Pill tone={on ? "ok" : "neutral"}>{on ? t.integrationOn : t.integrationOff}</Pill>
        </span>
      </div>
      {!monitoringOn && on && <div className="banner banner-warning">{t.integrationLeftBehind}</div>}
      {!on && <div className="help">{t.integrationExplain}</div>}
      {on && <div className="help mono">{view.enabled!.settingsPath}</div>}
      <div className="actions">
        {on ? (
          <button type="button" className="btn" onClick={disable} disabled={busy}>
            {t.disableIntegration}
          </button>
        ) : (
          monitoringOn && (
            <button type="button" className="btn btn-primary" onClick={() => setReview(true)} disabled={busy || !view.plan}>
              {t.reviewChange}
            </button>
          )
        )}
      </div>
      {view.planError && <div className="banner banner-warning">{view.planError}</div>}
      {message && <div className="help">{message}</div>}
      {review && view.plan && <ReviewDialog view={view} onCancel={() => setReview(false)} onEnable={enable} busy={busy} error={error} />}
    </div>
  );
}
