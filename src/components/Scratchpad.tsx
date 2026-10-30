import { useEffect, useMemo, useRef, useState } from "react";
import { draftHints, sendBlockReason } from "../terminal/scratchpadHints";
import { draftStore, type DraftStore } from "../terminal/drafts";
import { reportError } from "../uiErrors";

export type ScratchpadProps = {
  /** The pane (tab) id: one draft each. */
  paneId: string;
  /** Pastes the text into the pane's terminal, with no Enter or newline added. */
  onSend: (text: string) => void;
  /** Whether the terminal is in bracketed paste mode right now. */
  bracketedPaste: () => boolean;
  onClose: () => void;
  /** Tests give their own store. */
  store?: DraftStore;
};

/** A multi-line prompt editor under a terminal. Send pastes the draft; it never presses Enter. */
export function Scratchpad({ paneId, onSend, bracketedPaste, onClose, store }: ScratchpadProps) {
  const drafts = useMemo(() => store ?? draftStore((err) => reportError("Could not save the drafts", err, { once: "drafts" })), [store]);
  const [text, setText] = useState(() => drafts.get(paneId));
  const [history, setHistory] = useState<readonly string[]>(() => drafts.history());
  const area = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    let live = true;
    drafts.whenReady().then(() => {
      if (!live) return;
      setText((current) => current || drafts.get(paneId));
      setHistory(drafts.history());
    });
    area.current?.focus();
    return () => {
      live = false;
      drafts.flush();
    };
  }, [drafts, paneId]);

  const hints = draftHints(text);
  const blocked = sendBlockReason(text, bracketedPaste());
  const send = () => {
    if (blocked) return;
    onSend(text);
    drafts.sent(paneId, text);
    setText("");
    setHistory(drafts.history());
  };
  return (
    <section className="scratchpad" aria-label="Prompt scratchpad" data-testid="scratchpad">
      <textarea
        ref={area}
        className="scratchpad-text"
        rows={6}
        value={text}
        placeholder="Write the prompt here. Send pastes it into the terminal; you press Enter yourself."
        aria-label="Prompt draft"
        onChange={(event) => {
          setText(event.target.value);
          drafts.set(paneId, event.target.value);
        }}
        onBlur={() => drafts.flush()}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            onClose();
          }
        }}
      />
      {hints.length > 0 && (
        <ul className="hint scratchpad-hints" data-testid="scratchpad-hints">
          {hints.map((hint) => (
            <li key={hint}>{hint}</li>
          ))}
        </ul>
      )}
      <div className="scratchpad-actions">
        <button type="button" className="primary-button" disabled={blocked !== null} title={blocked ?? undefined} onClick={send}>
          Send to terminal
        </button>
        {blocked && text.trim() && (
          <span className="hint" role="status" data-testid="scratchpad-blocked">
            {blocked}
          </span>
        )}
        {history.length > 0 && (
          <select
            aria-label="Earlier prompts"
            value=""
            onChange={(event) => {
              const picked = history[Number(event.target.value)];
              if (picked !== undefined) {
                setText(picked);
                drafts.set(paneId, picked);
              }
            }}
          >
            <option value="">Earlier prompts…</option>
            {[...history]
              .map((entry, index) => ({ entry, index }))
              .reverse()
              .map(({ entry, index }) => (
                <option key={index} value={index}>
                  {entry.replace(/\s+/g, " ").slice(0, 60)}
                </option>
              ))}
          </select>
        )}
        <button type="button" className="link-button" onClick={onClose}>
          Close
        </button>
      </div>
    </section>
  );
}
