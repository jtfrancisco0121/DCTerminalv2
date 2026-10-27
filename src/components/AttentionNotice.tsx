import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { focusTab } from "../bridge";
import { reportError } from "../uiErrors";
import { InlineNotice } from "./InlineNotice";

/** The app-wide `attention` event (src-tauri/src/observe/notify.rs). */
export type AttentionEvent = {
  kind: string;
  tabId: string | null;
  windowLabel: string | null;
  runId: string | null;
  text: string;
};

/** At most this many notices stack; the oldest goes first. */
export const MAX_NOTICES = 3;
/** A notice goes by itself after this long. */
export const NOTICE_MS = 15_000;

type Notice = AttentionEvent & { id: number };

/**
 * What the detector sends while you are typing in some window: the same
 * words as the system notification, with Go to tab (which may switch
 * windows) and Dismiss. Every window shows it. macOS shows no banner while
 * the app is frontmost, so this is how the person is reached then.
 */
export function AttentionNotices() {
  const [notices, setNotices] = useState<Notice[]>([]);
  const next = useRef(0);

  const dismiss = useCallback((id: number) => setNotices((list) => list.filter((n) => n.id !== id)), []);

  const add = useCallback(
    (event: AttentionEvent) => {
      const id = (next.current += 1);
      setNotices((list) => [...list, { ...event, id }].slice(-MAX_NOTICES));
      window.setTimeout(() => dismiss(id), NOTICE_MS);
    },
    [dismiss],
  );

  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | null = null;
    listen<AttentionEvent>("attention", (event) => add(event.payload)).then(
      (fn) => (gone ? fn() : (unlisten = fn)),
      (err: unknown) => reportError("Notices", err, { once: true }),
    );
    return () => {
      gone = true;
      unlisten?.();
    };
  }, [add]);

  if (notices.length === 0) return null;
  return (
    <div className="attention-notices" aria-label="Notices">
      {notices.map((notice) => (
        <InlineNotice
          key={notice.id}
          tone="warning"
          action={
            <>
              {notice.tabId && (
                <button
                  type="button"
                  className="secondary-button"
                  onClick={() => {
                    focusTab(notice.tabId!).catch((err: unknown) => reportError("Go to tab", err, { notify: true }));
                    dismiss(notice.id);
                  }}
                >
                  Go to tab
                </button>
              )}{" "}
              <button type="button" className="secondary-button" onClick={() => dismiss(notice.id)}>
                Dismiss
              </button>
            </>
          }
        >
          {notice.text}
        </InlineNotice>
      ))}
    </div>
  );
}
