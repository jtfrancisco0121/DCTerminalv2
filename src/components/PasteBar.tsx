import { useEffect, useRef, useState } from "react";
import { compactText, estimateTokens, lineCount, onPasteOffer, type Compacted, type PasteOffer } from "../terminal/compactPaste";
import { errorText } from "./InlineNotice";

/**
 * The bar over a terminal for a big paste: sizes before and after, then
 * Paste compacted, Paste as is, or Esc (as is). Nothing is pasted until a choice.
 */
export function PasteBar({ tabId }: { tabId: string }) {
  const [offer, setOffer] = useState<PasteOffer | null>(null);
  const [compacted, setCompacted] = useState<Compacted | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const barRef = useRef<HTMLDivElement>(null);

  useEffect(
    () =>
      onPasteOffer((next) => {
        if (next.id !== tabId) return;
        setOffer(next);
        setCompacted(null);
        setProblem(null);
        compactText(next.text).then(setCompacted, (err: unknown) => setProblem(errorText(err)));
      }),
    [tabId],
  );

  useEffect(() => {
    if (offer) barRef.current?.focus();
  }, [offer]);

  if (!offer) return null;
  const done = () => setOffer(null);
  const asIs = () => {
    offer.paste(offer.text);
    done();
  };
  const compact = () => {
    if (!compacted) return;
    offer.paste(compacted.text);
    done();
  };
  return (
    <div
      ref={barRef}
      className="paste-bar"
      role="alertdialog"
      aria-label="Large paste"
      tabIndex={-1}
      data-testid="paste-bar"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          asIs();
        }
      }}
    >
      <span>
        Large paste: {lineCount(offer.text)} lines, about {estimateTokens(offer.text)} tokens.
        {compacted
          ? ` Compacted: ${compacted.linesAfter} lines, about ${estimateTokens(compacted.text)} tokens (error and warning lines kept).`
          : problem
            ? ` Could not compact it: ${problem}`
            : " Compacting…"}
      </span>
      <button type="button" className="secondary-button" disabled={!compacted} onClick={compact}>
        Paste compacted
      </button>
      <button type="button" className="secondary-button" onClick={asIs}>
        Paste as is
      </button>
      <span className="hint">Esc pastes as is</span>
    </div>
  );
}
