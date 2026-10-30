import { useEffect, useRef, useState } from "react";
import "@xterm/xterm/css/xterm.css";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { ptyResize, terminalResumeId, type TabSummary } from "../bridge";
import { detectPlatform, isAppShortcut, isScratchpadShortcut, isShiftEnter, SCRATCHPAD_EVENT, SHIFT_ENTER_SEQUENCE, shiftEnterSendsNewline } from "../terminal/keys";
import { dropPointToCss, formatDroppedPaths } from "../terminal/drop";
import { firstOpenResumes, folderProblem } from "../terminal/openPlan";
import { FolderMissing } from "./FolderMissing";
import { TerminalSearchBar } from "./TerminalSearchBar";
import { PasteBar } from "./PasteBar";
import { Scratchpad } from "./Scratchpad";
import { TerminalExitStrip, lastFiftyLines, lastOutputLine } from "./TerminalExitStrip";
import { TerminalContextMenu, type ContextMenuItem } from "./TerminalContextMenu";
import { reportError } from "../uiErrors";
import { isPtyOpening, livePty, openLivePty, sendPtyInput, subscribeLivePty } from "../terminal/live";
import {
  blurParkedTerminal,
  clearTerminalScrollback,
  copyTerminalSelection,
  ensureParkedTerminal,
  focusParkedTerminal,
  isBracketedPasteOn,
  isParkedHost,
  measuredTerminalSize,
  noteTerminalSize,
  pasteIntoTerminal,
  pasteTerminalText,
  pasteWithOffer,
  releaseParkedTerminal,
  setTerminalRefitter,
  setTerminalSearchOpener,
  terminalScrollbackText,
  terminalSelection,
  type ParkedTerminal,
} from "../terminal/park";

export type TerminalViewProps = {
  tab: TabSummary;
  fontSize: number;
  /** Take keyboard focus when shown (the active tab). */
  autoFocus?: boolean;
  /** The tab was pointed at another folder (the window reads its tabs again). */
  onFolderChanged?: () => void;
};

/**
 * The size to open a PTY at: the mounted slot, measured now, so the program
 * draws its first screen at the right width. Before any slot exists, the last
 * measured terminal (else 80x24); the next fit corrects it.
 */
function openSize(parked: ParkedTerminal): { cols: number; rows: number } {
  const slot = parked.host.parentElement;
  if (parked.host.isConnected && slot && slot.clientWidth > 0 && slot.clientHeight > 0) {
    try {
      parked.fit.fit();
    } catch {
      // Fall through to whatever xterm currently has.
    }
    if (parked.term.cols >= 2 && parked.term.rows >= 1) {
      noteTerminalSize(parked.term.cols, parked.term.rows);
      return { cols: parked.term.cols, rows: parked.term.rows };
    }
  }
  return measuredTerminalSize();
}

/** True when keyboard focus is in a dialog or the settings page, which must keep it. */
function focusBelongsElsewhere(): boolean {
  const active = document.activeElement;
  if (!active || active === document.body) return false;
  return !!active.closest(".dialog, .settings-page");
}

/**
 * One tab's terminal. The first time the tab is shown in this page its PTY
 * opens (a Claude tab resumes its session when it can); after that the xterm
 * and the PTY outlive the view, parked while another tab is shown.
 */
export function TerminalView({ tab, fontSize, autoFocus = true, onFolderChanged }: TerminalViewProps) {
  const ptyId = tab.id;
  const claude = tab.kind === "claude";
  const [padOpen, setPadOpen] = useState(false);
  const slotRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<HTMLDivElement>(null);
  const [exitCode, setExitCode] = useState<number | null>(livePty(ptyId)?.exitCode ?? null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [menu, setMenu] = useState<{ x: number; y: number; hasSelection: boolean } | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [dropOver, setDropOver] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const [resumeId, setResumeId] = useState<string | null | undefined>(undefined);
  const [lastLine, setLastLine] = useState("");
  const [copied, setCopied] = useState(false);
  const problem = folderProblem(tab);

  // Once a Claude terminal exits (or fails to open): is there a session to
  // resume, and what did it print last.
  // Cmd/Ctrl+Shift+E and the palette entry toggle the scratchpad of the pane that has focus (else the active one).
  useEffect(() => {
    const mine = () => viewRef.current?.contains(document.activeElement) === true || (autoFocus && !document.querySelector(".terminal-view:focus-within"));
    const onKey = (event: KeyboardEvent) => {
      if (!isScratchpadShortcut(event, detectPlatform(navigator.platform)) || !mine()) return;
      event.preventDefault();
      setPadOpen((open) => !open);
    };
    const onToggle = () => {
      if (mine()) setPadOpen((open) => !open);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener(SCRATCHPAD_EVENT, onToggle);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener(SCRATCHPAD_EVENT, onToggle);
    };
  }, [autoFocus]);

  useEffect(() => {
    if (!claude || (exitCode == null && !failed)) return;
    let current = true;
    setResumeId(undefined);
    setCopied(false);
    setLastLine(lastOutputLine(terminalScrollbackText(ptyId)));
    terminalResumeId(ptyId)
      .then((id) => current && setResumeId(id ?? null))
      .catch(() => current && setResumeId(null));
    return () => {
      current = false;
    };
  }, [claude, exitCode, failed, ptyId]);

  // Escape closes the right-click menu. Capture it so the program in the
  // terminal does not also see the key.
  useEffect(() => {
    if (!menu) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setMenu(null);
      focusParkedTerminal(ptyId);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [menu, ptyId]);

  // When focus moves outside the menu and its dismiss button, close it, so
  // Escape is never left half-owned by a closed menu.
  useEffect(() => {
    if (!menu) return;
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target as Element | null;
      if (target?.closest?.(".terminal-menu, .terminal-menu-dismiss")) return;
      setMenu(null);
    };
    document.addEventListener("focusin", onFocusIn);
    return () => document.removeEventListener("focusin", onFocusIn);
  }, [menu]);

  // Files dropped from the OS paste their quoted paths, like a paste. Tauri
  // reports the drop for the whole window, so each view checks the position.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const over = (position: { x: number; y: number }) => {
      // Tauri reports physical pixels; the hit test wants CSS pixels.
      const point = dropPointToCss(position, window.devicePixelRatio);
      const target = document.elementFromPoint(point.x, point.y);
      return !!target && !!viewRef.current?.contains(target);
    };
    try {
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type === "leave") {
            setDropOver(false);
          } else if (payload.type === "drop") {
            setDropOver(false);
            if (!over(payload.position)) return;
            const text = formatDroppedPaths(payload.paths, detectPlatform(navigator.platform) === "windows");
            pasteIntoTerminal(ptyId, text);
            ensureParkedTerminal(ptyId, fontSize).term.focus();
            focusParkedTerminal(ptyId);
          } else {
            setDropOver(over(payload.position));
          }
        })
        .then((fn) => {
          if (disposed) fn();
          else unlisten = fn;
        })
        // Background: dropping files then does nothing, so say why in the log.
        .catch((err: unknown) => reportError("File drop unavailable", err, { once: true }));
    } catch {
      // Not running inside Tauri (unit tests, a plain browser).
    }
    return () => {
      disposed = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ptyId]);

  // Cmd+V with only an image on the clipboard types nothing; say how to attach it.
  useEffect(() => {
    if (!claude || detectPlatform(navigator.platform) !== "mac") return;
    const host = ensureParkedTerminal(ptyId, fontSize).host;
    let timer = 0;
    const onPaste = (event: ClipboardEvent) => {
      const data = event.clipboardData;
      if (!data || data.getData("text/plain")) return;
      const hasImage = Array.from(data.items ?? []).some(
        (item) => item.kind === "file" && item.type.startsWith("image/"),
      );
      if (!hasImage) return;
      setHint("Press Ctrl+V to attach images in Claude Code");
      window.clearTimeout(timer);
      timer = window.setTimeout(() => setHint(null), 5000);
    };
    host.addEventListener("paste", onPaste, true);
    return () => {
      window.clearTimeout(timer);
      host.removeEventListener("paste", onPaste, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [claude, ptyId]);

  useEffect(() => subscribeLivePty(() => setGeneration((value) => value + 1)), []);

  // The parked terminal feeds xterm from the PTY; this view only shows
  // whether the program exited (a restart replaces the session).
  useEffect(() => {
    const live = livePty(ptyId);
    setExitCode(live?.exited ? (live.exitCode ?? -1) : null);
  }, [ptyId, generation]);

  useEffect(() => setTerminalSearchOpener(ptyId, () => setSearchOpen(true)), [ptyId]);

  useEffect(() => {
    const slot = slotRef.current;
    if (!slot || problem) return;
    const parked = ensureParkedTerminal(ptyId, fontSize);
    slot.appendChild(parked.host);
    parked.term.options.fontSize = fontSize;
    if (autoFocus && !focusBelongsElsewhere()) {
      parked.term.focus();
      focusParkedTerminal(ptyId);
    }

    const platform = detectPlatform(navigator.platform);
    const newlineOnShiftEnter = shiftEnterSendsNewline(tab.kind);
    parked.term.attachCustomKeyEventHandler((event) => {
      if (newlineOnShiftEnter && isShiftEnter(event)) {
        // Handle keydown only; swallow keypress and keyup so xterm sends no CR.
        if (event.type === "keydown" && !event.repeat) sendPtyInput(ptyId, SHIFT_ENTER_SEQUENCE);
        return false;
      }
      // The app's own shortcuts bubble to the window, unsent to the program.
      // Cmd/Ctrl+Shift+E opens the scratchpad: the window listener handles it, not the program.
      return !isAppShortcut(event, platform) && !isScratchpadShortcut(event, platform);
    });

    const fitNow = () => {
      // A hidden or collapsed slot has no size. Fitting then would shrink
      // the PTY to the minimum and garble the program's output.
      if (!parked.host.isConnected || slot.clientWidth === 0 || slot.clientHeight === 0) return;
      parked.fit.fit();
      const cols = parked.term.cols;
      const rows = parked.term.rows;
      if (cols >= 2 && rows >= 1) {
        noteTerminalSize(cols, rows);
        void ptyResize(ptyId, cols, rows).catch(() => {});
      }
    };
    // Coalesce bursts (window drags) into one fit per frame, after layout has settled.
    let frame = 0;
    const scheduleFit = () => {
      if (frame) return;
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        fitNow();
      });
    };
    const observer = new ResizeObserver(() => scheduleFit());
    observer.observe(slot);
    window.addEventListener("resize", scheduleFit);
    const unregisterRefit = setTerminalRefitter(ptyId, scheduleFit);
    const timer = window.setTimeout(fitNow, 0);

    return () => {
      window.clearTimeout(timer);
      if (frame) window.cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", scheduleFit);
      unregisterRefit();
      blurParkedTerminal(ptyId);
      const root = document.getElementById("terminal-park");
      // A closed tab already released its terminal; don't park the disposed host.
      if (root && isParkedHost(ptyId, parked.host) && parked.host.parentElement !== root) {
        root.appendChild(parked.host);
      }
    };
  }, [autoFocus, ptyId, fontSize, generation, tab.kind, problem]);

  // The first show in this page opens the PTY; a Claude tab that ran before
  // resumes its session when it still can. The open belongs to the tab, not
  // this view: a remount meanwhile (StrictMode) finds it opening and waits.
  useEffect(() => {
    if (problem) return;
    void openLivePty(ptyId, {
      ifIdle: true,
      before: () => firstOpenResumes(tab, terminalResumeId),
      start: (resume) => ({ ...openSize(ensureParkedTerminal(ptyId, fontSize)), resume }),
      onError: setFailed,
    }).then((outcome) => {
      // The tab was closed while the open was starting: it opened nothing, so
      // leave no xterm behind, unless the same id was reopened since.
      if (outcome === "closed" && !isPtyOpening(ptyId) && !livePty(ptyId)) releaseParkedTerminal(ptyId);
    });
    // The tab record only matters for this first open.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ptyId, problem]);

  const restart = (resume: boolean) => {
    setExitCode(null);
    setFailed(null);
    const parked = ensureParkedTerminal(ptyId, fontSize);
    parked.term.reset();
    void openLivePty(ptyId, {
      start: () => ({ ...openSize(parked), resume }),
      onError: setFailed,
    });
  };

  const mac = detectPlatform(navigator.platform) === "mac";
  const menuItems = (hasSelection: boolean): ContextMenuItem[] => {
    const close = (run: () => void) => () => {
      setMenu(null);
      run();
    };
    return [
      {
        id: "copy",
        label: "Copy",
        shortcut: mac ? "⌘C" : "Ctrl+Shift+C",
        disabled: !hasSelection,
        onSelect: close(() => void copyTerminalSelection(ptyId)),
      },
      {
        id: "paste",
        label: "Paste",
        shortcut: mac ? "⌘V" : "Ctrl+Shift+V",
        onSelect: close(() => void pasteTerminalText(ptyId)),
      },
      {
        id: "find",
        label: "Find…",
        shortcut: mac ? "⌘F" : "Ctrl+Shift+F",
        onSelect: close(() => setSearchOpen(true)),
      },
      {
        id: "clear",
        label: "Clear scrollback",
        divider: true,
        onSelect: close(() => clearTerminalScrollback(ptyId)),
      },
    ];
  };

  if (problem) {
    return <FolderMissing tabId={tab.id} problem={problem} onChanged={() => onFolderChanged?.()} />;
  }

  const ended = failed != null || exitCode != null;
  return (
    <div
      ref={viewRef}
      className={dropOver ? "terminal-view terminal-drop-over" : "terminal-view"}
      onContextMenu={(event) => {
        event.preventDefault();
        setMenu({
          x: event.clientX,
          y: event.clientY,
          hasSelection: terminalSelection(ptyId).length > 0,
        });
      }}
    >
      {claude && ended && (
        <TerminalExitStrip
          exitCode={exitCode}
          lastLine={lastLine}
          failed={failed}
          resumeId={resumeId}
          copied={copied}
          onResume={() => restart(true)}
          onFresh={() => restart(false)}
          onCopy={() => {
            void navigator.clipboard
              .writeText(lastFiftyLines(terminalScrollbackText(ptyId)))
              .then(() => setCopied(true))
              .catch((err: unknown) => reportError("Copy failed", err, { notify: true }));
          }}
        />
      )}
      {searchOpen && (
        <TerminalSearchBar
          search={ensureParkedTerminal(ptyId, fontSize).search}
          onClose={() => {
            setSearchOpen(false);
            ensureParkedTerminal(ptyId, fontSize).term.focus();
            focusParkedTerminal(ptyId);
          }}
        />
      )}
      {padOpen && (
        <Scratchpad
          paneId={ptyId}
          bracketedPaste={() => isBracketedPasteOn(ptyId)}
          onSend={(text) => pasteWithOffer(ptyId, text)}
          onClose={() => {
            setPadOpen(false);
            focusParkedTerminal(ptyId);
          }}
        />
      )}
      <PasteBar tabId={ptyId} />
      <div ref={slotRef} className="terminal-slot" onMouseDown={() => focusParkedTerminal(ptyId)} />
      {!claude && ended && (
        <div className="terminal-exit">
          {failed && <p className="error">{failed}</p>}
          {exitCode != null && <p>Process exited ({exitCode}).</p>}
          <button type="button" className="primary-button" onClick={() => restart(false)}>
            Restart
          </button>
        </div>
      )}
      {hint && (
        <div className="terminal-hint" role="status">
          {hint}
        </div>
      )}
      {menu && <TerminalContextMenu x={menu.x} y={menu.y} items={menuItems(menu.hasSelection)} />}
      {menu && (
        <button
          type="button"
          className="terminal-menu-dismiss"
          aria-label="Close menu"
          data-own-escape=""
          onClick={() => setMenu(null)}
        />
      )}
    </div>
  );
}

/** A closed tab's xterm goes away with it. */
export function destroyTerminal(ptyId: string): void {
  releaseParkedTerminal(ptyId);
}
