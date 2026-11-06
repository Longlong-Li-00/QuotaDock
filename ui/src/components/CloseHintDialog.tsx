import { useEffect, useRef, useState } from "react";
import { useApp } from "../context.ts";

/** Shown the first time the window is closed while "Close to tray" is on. */
export function CloseHintDialog({ onDone }: { onDone: (dontShowAgain: boolean) => void }) {
  const { t } = useApp();
  const [dontShow, setDontShow] = useState(false);
  const ok = useRef<HTMLButtonElement>(null);
  useEffect(() => ok.current?.focus(), []);
  return (
    <div className="modal-backdrop">
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="close-hint-title">
        <h2 id="close-hint-title">{t.closeHintTitle}</h2>
        <p>{t.closeHintBody}</p>
        <label className="check">
          <input type="checkbox" checked={dontShow} onChange={(e) => setDontShow(e.target.checked)} />
          {t.dontShowAgain}
        </label>
        <div className="actions">
          <button ref={ok} type="button" className="btn btn-primary" onClick={() => onDone(dontShow)}>
            {t.ok}
          </button>
        </div>
      </div>
    </div>
  );
}
