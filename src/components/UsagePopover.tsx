import { useEffect, useState } from "react";
import { costText, observeTab, tokensText, type TabObservation } from "../run/observe";

type Props = {
  tabId: string;
  onClose: () => void;
};

/** What a plain Claude tab's session has used so far, from its session log. */
export function UsagePopover({ tabId, onClose }: Props) {
  const [seen, setSeen] = useState<TabObservation | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let live = true;
    observeTab(tabId).then(
      (value) => live && setSeen(value),
      () => live && setFailed(true),
    );
    return () => {
      live = false;
    };
  }, [tabId]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const usage = seen?.usage;
  return (
    <div className="usage-popover" role="dialog" aria-label="Session usage">
      <button type="button" className="usage-popover-close" aria-label="Close usage" onClick={onClose}>
        ×
      </button>
      {failed && <p className="hint">Usage is not available for this tab.</p>}
      {!failed && !seen && <p className="hint">Reading the session…</p>}
      {seen && (!usage || usage.messages === 0) && <p className="hint">No usage recorded for this session yet.</p>}
      {usage && usage.messages > 0 && (
        <>
          <p data-testid="usage-tokens">{tokensText(usage.totalTokens)} tokens</p>
          <p data-testid="usage-cost">{costText(usage.totalCostUsd)} estimate</p>
          <p data-testid="usage-minutes">{usage.minutes.toFixed(0)} min</p>
        </>
      )}
    </div>
  );
}
