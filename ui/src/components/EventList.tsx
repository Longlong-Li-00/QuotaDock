import { useState } from "react";
import type { EventView } from "../api/types.ts";
import { useApp } from "../context.ts";
import { beforeAfter, confidenceLabel, eventTitle, evidenceLines, formatDateTime, technicalName, windowName } from "../model/present.ts";
import { InfoTip } from "./common.tsx";

const PROVIDER_NAME: Record<string, string> = { codex: "Codex", claude: "Claude" };

function EventItem({ e }: { e: EventView }) {
  const { t, lang } = useApp();
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const ba = beforeAfter(e, t, lang);
  const lines = evidenceLines(e, t);
  const diagnostic = e.visibility === "diagnostic";
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(JSON.stringify({ provider: e.provider, eventType: e.eventType, window: e.windowType, observedAtMs: e.observedAtMs, confidence: e.confidence, evidence: e.evidence }, null, 2));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };
  return (
    <li className={`event ${diagnostic ? "event-diag" : ""}`}>
      <button type="button" className="event-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="event-title">
          <span className={`provider-tag provider-tag-${e.provider}`}>{PROVIDER_NAME[e.provider] ?? e.provider}</span> · {eventTitle(e, t)}
        </span>
        <span className="event-meta">
          {windowName(e.windowType, t)} · {formatDateTime(e.observedAtMs, lang)}
        </span>
      </button>
      {open && (
        <div className="event-body">
          {diagnostic && <div className="tech">{technicalName(e.eventType)}</div>}
          <dl className="kvs">
            <dt>{t.observedAt}</dt>
            <dd>{formatDateTime(e.observedAtMs, lang)}</dd>
            <dt>{t.windowLabel}</dt>
            <dd>{windowName(e.windowType, t)}</dd>
            {ba && (
              <>
                <dt>{t.before}</dt>
                <dd>{ba.before.join(" · ")}</dd>
                <dt>{t.after}</dt>
                <dd>{ba.after.join(" · ")}</dd>
              </>
            )}
            {lines.length > 0 && (
              <>
                <dt>{t.evidence}</dt>
                <dd>
                  <ul className="evidence">
                    {lines.map((l) => (
                      <li key={l}>{l}</li>
                    ))}
                  </ul>
                </dd>
              </>
            )}
            <dt>
              {t.confidence} <InfoTip text={t.confidenceTip} />
            </dt>
            <dd>{confidenceLabel(e.confidence, t)}</dd>
          </dl>
          {diagnostic && (
            <button type="button" className="btn btn-small" onClick={copy}>
              {copied ? t.copied : t.copyDetails}
            </button>
          )}
        </div>
      )}
    </li>
  );
}

export function EventList({ events }: { events: EventView[] }) {
  return (
    <ul className="events">
      {events.map((e) => (
        <EventItem key={`${e.provider}-${e.id}`} e={e} />
      ))}
    </ul>
  );
}
