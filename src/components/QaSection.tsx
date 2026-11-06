import { useEffect, useRef, useState } from "react";
import { runQaImage, type PrView, type QaCriterion } from "../run/api";
import { qaPill } from "../run/words";
import { Dialog } from "./Dialog";
import { Pill } from "./Pill";

/** At most this many thumbnails are drawn; the rest are a "+n more" count. */
const MAX_THUMBS = 6;

/** The screenshot as a data URL, loaded once `enabled` (lazily, when it scrolls into view); "refused" when the app says no. */
function useQaImage(runDir: string, name: string, enabled: boolean) {
  const [src, setSrc] = useState<string | "refused" | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let live = true;
    runQaImage(runDir, name).then(
      (url) => live && setSrc(url),
      () => live && setSrc("refused"),
    );
    return () => {
      live = false;
    };
  }, [runDir, name, enabled]);
  return src;
}

function Thumb({ runDir, criterion, onOpen }: { runDir: string; criterion: QaCriterion; onOpen: () => void }) {
  const ref = useRef<HTMLButtonElement>(null);
  const [seen, setSeen] = useState(typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (seen || !ref.current) return;
    const watcher = new IntersectionObserver((entries) => entries.some((e) => e.isIntersecting) && setSeen(true));
    watcher.observe(ref.current);
    return () => watcher.disconnect();
  }, [seen]);
  const src = useQaImage(runDir, criterion.image ?? "", seen && !!criterion.image);
  const label = `Screenshot of ${criterion.id}`;
  return (
    <button ref={ref} type="button" className="qa-thumb" aria-label={label} onClick={onOpen}>
      {src && src !== "refused" ? <img src={src} alt={`${criterion.id}: ${criterion.text}`} /> : <span>{src === "refused" ? "image unavailable" : "…"}</span>}
    </button>
  );
}

/** The full screenshot, over the run panel: Esc closes, Left and Right step, focus goes back to the thumbnail (Dialog). */
function ShotDialog({ runDir, shots, start, onClose }: { runDir: string; shots: QaCriterion[]; start: number; onClose: () => void }) {
  const [at, setAt] = useState(start);
  const shot = shots[at];
  const src = useQaImage(runDir, shot.image ?? "", true);
  const step = (by: number) => setAt((n) => (n + by + shots.length) % shots.length);
  return (
    <Dialog title={`Screenshot of ${shot.id}`} onClose={onClose} className="qa-shot-dialog">
      <div
        className="qa-shot"
        onKeyDown={(event) => {
          if (event.key === "ArrowLeft") {
            event.preventDefault();
            step(-1);
          } else if (event.key === "ArrowRight") {
            event.preventDefault();
            step(1);
          }
        }}
      >
        <div className="qa-shot-bar">
          <button type="button" className="secondary-button" onClick={() => step(-1)} disabled={shots.length < 2}>
            Previous
          </button>
          <span>
            {shot.id} · {shot.result} · {at + 1} of {shots.length}
          </span>
          <button type="button" className="secondary-button" onClick={() => step(1)} disabled={shots.length < 2}>
            Next
          </button>
        </div>
        {src && src !== "refused" ? <img src={src} alt={`${shot.id}: ${shot.text}`} className="qa-shot-image" /> : <p className="hint">{src === "refused" ? "image unavailable" : "Loading…"}</p>}
        <p className="hint">{shot.text}</p>
      </div>
    </Dialog>
  );
}

/**
 * A PR's QA result in the run panel: a pill, the criterion rows (failed first) and up to six screenshot thumbnails.
 * Nothing at all for a PR with no QA result that is not being tried.
 */
export function QaSection({ runDir, pr }: { runDir: string; pr: PrView }) {
  const pill = qaPill(pr);
  const [open, setOpen] = useState<number | null>(null);
  if (!pill) return null;
  const criteria = pr.qa?.criteria ?? [];
  const shots = criteria.filter((c) => c.image);
  const shown = shots.slice(0, MAX_THUMBS);
  return (
    <div className="run-pr-qa" data-testid={`qa-${pr.id}`}>
      <Pill tone={pill.tone} testId={`qa-pill-${pr.id}`}>
        {pill.text}
      </Pill>
      {criteria.length > 0 && (
        <ul className="qa-criteria">
          {criteria.map((c) => (
            <li key={c.id} className={`qa-criterion qa-criterion-${c.result.toLowerCase()}`}>
              <strong>{c.id}</strong> {c.result === "PASS" ? "✓" : c.result === "FAIL" ? "✗" : "?"} {c.text}
            </li>
          ))}
        </ul>
      )}
      {shown.length > 0 && (
        <div className="qa-thumbs">
          {shown.map((c, at) => (
            <Thumb key={c.id} runDir={runDir} criterion={c} onOpen={() => setOpen(at)} />
          ))}
          {shots.length > shown.length && <span className="hint">+{shots.length - shown.length} more</span>}
        </div>
      )}
      {open !== null && <ShotDialog runDir={runDir} shots={shown} start={open} onClose={() => setOpen(null)} />}
    </div>
  );
}
