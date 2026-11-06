import { useId, type ReactNode } from "react";
import type { DisplayTone } from "../api/types.ts";
import type { Tone } from "../model/present.ts";
import { useApp } from "../context.ts";

/** Accessible "i" tooltip: keyboard focusable, text announced via aria-describedby. */
export function InfoTip({ text }: { text: string }) {
  const id = useId();
  const { t } = useApp();
  return (
    <span className="info">
      <button type="button" className="info-btn" aria-label={t.info} aria-describedby={id}>
        i
      </button>
      <span role="tooltip" id={id} className="info-pop">
        {text}
      </span>
    </span>
  );
}

/** Status badge: colour is never the only signal, the text always carries the meaning. */
export function Pill({ tone, children, title }: { tone: Tone; children: ReactNode; title?: string }) {
  return (
    <span className={`pill pill-${tone}`} title={title}>
      <span className="pill-dot" aria-hidden="true" />
      {children}
    </span>
  );
}

export function EstimateBadge() {
  const { t } = useApp();
  return (
    <span className="estimate">
      {t.estimateBadge} <InfoTip text={t.estimateTip} />
    </span>
  );
}

export function Toggle({ label, help, checked, onChange, disabled }: { label: string; help?: string; checked: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  const id = useId();
  return (
    <div className="toggle-row">
      <div className="toggle-text">
        <label htmlFor={id}>{label}</label>
        {help && <div className="help">{help}</div>}
      </div>
      <input id={id} type="checkbox" role="switch" className="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
    </div>
  );
}

export function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="card section">
      <h2>{title}</h2>
      {children}
    </section>
  );
}

export function Meter({ value, tone, label }: { value: number | null; tone: DisplayTone; label: string }) {
  const v = value === null ? 0 : Math.max(0, Math.min(100, value));
  return (
    <div className="meter" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={value ?? undefined}>
      <div className={`meter-fill meter-${tone}`} style={{ width: `${v}%` }} />
    </div>
  );
}
