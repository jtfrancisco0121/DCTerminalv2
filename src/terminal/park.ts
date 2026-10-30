/**
 * xterm instances live outside React. Tab switches reparent the host node
 * into a hidden park so the scrollback survives an unmount (and the PTY
 * behind it keeps running).
 */

import { pasteOrOffer, shouldOffer } from "./compactPaste";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { Terminal } from "@xterm/xterm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { lastLines, TERMINAL_TAIL_LINES } from "./text";
import { ptySetScheme } from "../bridge";
import { cachedTheme, isLightTheme, terminalTheme } from "../theme";
import { detectPlatform } from "./keys";
import { isLinkModifier } from "./links";
import { reportError } from "../uiErrors";
import { livePty, sendPtyInput, subscribeLivePty, type LivePty } from "./live";

export type ParkedTerminal = {
  host: HTMLDivElement;
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
};

const parked = new Map<string, ParkedTerminal>();
let parkRoot: HTMLDivElement | null = null;
let focusedId = "";
/** Stops what feeds a parked terminal: its input and its PTY output. */
const streams = new Map<string, () => void>();
const searchOpeners = new Map<string, () => void>();
const refitters = new Map<string, () => void>();

let currentTheme = terminalTheme(cachedTheme());

/** Repaint every terminal (and future ones) in the app theme. */
export function setTerminalTheme(id: string): void {
  currentTheme = terminalTheme(id);
  const light = isLightTheme(id);
  try {
    void ptySetScheme(light).catch(() => {});
  } catch {
    // No Tauri bridge (unit tests, plain browser): nothing to tell.
  }
  for (const entry of parked.values()) entry.term.options.theme = currentTheme;
}

/** Bundled with the app (@fontsource-variable/jetbrains-mono, loaded in main.tsx). */
const BUNDLED_FONT = '"JetBrains Mono Variable", ';
const MAC_FONTS = '"SF Mono", SFMono-Regular, Menlo, ';
const FONTS = '"Cascadia Code", "Cascadia Mono", Consolas, ui-monospace, monospace';

/** The bundled JetBrains Mono first; then SF Mono and Menlo on macOS, the Windows stack elsewhere. */
export function terminalFontFamily(platform: string): string {
  return BUNDLED_FONT + (/mac/i.test(platform) ? MAC_FONTS + FONTS : FONTS);
}

let fontWatchStarted = false;

/**
 * xterm measures the cell size when it opens. A terminal opened before the
 * bundled font finished loading measured a fallback font, so once the font
 * is ready every open terminal re-measures and refits.
 */
function watchBundledFont(): void {
  if (fontWatchStarted) return;
  fontWatchStarted = true;
  try {
    void document.fonts
      ?.load('14px "JetBrains Mono Variable"')
      .then(() => {
        for (const [id, entry] of parked) {
          const family = entry.term.options.fontFamily ?? terminalFontFamily(navigator.platform);
          entry.term.options.fontFamily = "monospace";
          entry.term.options.fontFamily = family;
          refitters.get(id)?.();
        }
      })
      .catch(() => {});
  } catch {
    // No font loading API (unit tests): fallbacks are measured as usual.
  }
}

function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
  } catch {
    return false;
  }
}

/** Size of the last terminal that was fitted to a real slot. */
let lastFitSize: { cols: number; rows: number } | null = null;

export function noteTerminalSize(cols: number, rows: number): void {
  if (cols >= 2 && rows >= 1) lastFitSize = { cols, rows };
}

/** Where a new PTY should open: the measured slot size, else 80x24 until the first fit. */
export function measuredTerminalSize(): { cols: number; rows: number } {
  return lastFitSize ?? { cols: 80, rows: 24 };
}

function parkHost(): HTMLDivElement {
  if (parkRoot && parkRoot.isConnected) return parkRoot;
  const root = document.createElement("div");
  root.id = "terminal-park";
  root.setAttribute("aria-hidden", "true");
  root.style.cssText = "position:fixed;left:-10000px;top:0;width:0;height:0;overflow:hidden;";
  document.body.appendChild(root);
  parkRoot = root;
  return root;
}

let optionAsMeta = true;

/** macOS: Option acts as Meta. Applies live to every open terminal. */
export function setOptionAsMeta(value: boolean): void {
  optionAsMeta = value;
  for (const entry of parked.values()) entry.term.options.macOptionIsMeta = value;
}

let compactPasteOn = true;

/** Offer to compact big pastes (settings: `terminal.compactPaste`). */
export function setCompactPaste(value: boolean): void {
  compactPasteOn = value;
}

let copyOnSelect = false;

/** Copy a finished selection to the clipboard. Off by default. */
export function setCopyOnSelect(value: boolean): void {
  copyOnSelect = value;
}

export function ensureParkedTerminal(id: string, fontSize: number): ParkedTerminal {
  const existing = parked.get(id);
  if (existing) {
    existing.term.options.fontSize = fontSize;
    return existing;
  }
  watchBundledFont();
  const host = document.createElement("div");
  host.className = "xterm-host";
  const term = new Terminal({
    fontFamily: terminalFontFamily(navigator.platform),
    fontSize,
    scrollback: 10000,
    theme: currentTheme,
    cursorBlink: !prefersReducedMotion(),
    macOptionIsMeta: optionAsMeta,
    // Dim and low-contrast ANSI colors are lifted to a readable ratio.
    minimumContrastRatio: 4.5,
    rightClickSelectsWord: false,
    // The search addon's match highlights use the decoration API.
    allowProposedApi: true,
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(
    // Cmd-click (Ctrl-click elsewhere) opens a URL. A plain click only selects.
    new WebLinksAddon((event, uri) => {
      if (!isLinkModifier(event, detectPlatform(navigator.platform))) return;
      void openUrl(uri).catch((err: unknown) => reportError("Could not open link", err, { notify: true }));
    }),
  );
  let copyTimer = 0;
  term.onSelectionChange(() => {
    if (!copyOnSelect) return;
    window.clearTimeout(copyTimer);
    // Wait for the drag to settle, then copy once.
    copyTimer = window.setTimeout(() => {
      if (!term.hasSelection()) return;
      // Every selection copies, so a denied clipboard is shown once per terminal, not per drag.
      void navigator.clipboard
        .writeText(term.getSelection())
        .catch((err: unknown) =>
          reportError("Copy on select failed", err, { notify: true, once: `Copy on select failed:${id}` }),
        );
    }, 200);
  });
  term.open(host);
  // A big paste goes to the paste bar first (nothing is altered unless the user picks Paste compacted).
  term.textarea?.addEventListener(
    "paste",
    (event) => {
      const text = (event as ClipboardEvent).clipboardData?.getData("text") ?? "";
      if (!shouldOffer(text, compactPasteOn)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      pasteOrOffer(id, text, compactPasteOn, (t) => term.paste(t));
    },
    true,
  );
  streams.set(id, connectStreams(id, term));
  parkHost().appendChild(host);
  const created = { host, term, fit, search };
  parked.set(id, created);
  return created;
}

/**
 * The parked terminal, not a mounted view, owns its streams: keystrokes go
 * to the PTY through `live.ts`, and output goes in from the tab's live
 * buffer, so a hidden tab keeps draining into xterm's scrollback and still
 * answers terminal queries (DSR) instead of piling bytes up. Subscribed
 * before output is replayed: ConPTY's first bytes are often a cursor query,
 * and xterm answers only through onData. A Restart or Resume replaces the
 * live session, so the output is re-attached whenever it changes.
 */
function connectStreams(id: string, term: Terminal): () => void {
  const input = term.onData((data) => sendPtyInput(id, data));
  let attached: LivePty | undefined;
  let detach: (() => void) | undefined;
  const sync = () => {
    const live = livePty(id);
    if (live === attached) return;
    detach?.();
    attached = live;
    detach = live?.buffer.attach({ data: (bytes) => term.write(bytes), exit: () => {} });
  };
  sync();
  const unsubscribe = subscribeLivePty(sync);
  return () => {
    unsubscribe();
    detach?.();
    input.dispose();
  };
}

/** False once the terminal was released (its tab closed). */
export function isParkedHost(id: string, host: HTMLElement): boolean {
  return parked.get(id)?.host === host;
}

export function releaseParkedTerminal(id: string): void {
  const entry = parked.get(id);
  if (!entry) return;
  streams.get(id)?.();
  streams.delete(id);
  entry.term.dispose();
  entry.host.remove();
  parked.delete(id);
  searchOpeners.delete(id);
  refitters.delete(id);
  if (focusedId === id) focusedId = "";
}

export function focusParkedTerminal(id: string): void {
  focusedId = id;
}

/** A parked terminal must not keep keyboard focus after its tab is hidden. */
export function blurParkedTerminal(id: string): void {
  const entry = parked.get(id);
  entry?.term.blur();
  if (focusedId === id) focusedId = "";
}

export function setTerminalSearchOpener(id: string, open: () => void): () => void {
  searchOpeners.set(id, open);
  return () => {
    if (searchOpeners.get(id) === open) searchOpeners.delete(id);
  };
}

/** Fits the terminal to its slot now (the slot's size changed without a window resize). */
export function refitTerminal(id: string): void {
  refitters.get(id)?.();
}

/** The mounted view registers how to refit its xterm to the slot. */
export function setTerminalRefitter(id: string, refit: () => void): () => void {
  refitters.set(id, refit);
  return () => {
    if (refitters.get(id) === refit) refitters.delete(id);
  };
}

/** Open the search bar of terminal `id`, or the focused (else first) one. */
export function requestTerminalSearch(target?: string | null): void {
  const id = target || focusedId || parked.keys().next().value;
  if (id) searchOpeners.get(id)?.();
}

export function terminalSelection(id: string): string {
  return parked.get(id)?.term.getSelection().trim() ?? "";
}

export function terminalTailText(id: string): string {
  const term = parked.get(id)?.term;
  if (!term) return "";
  const buffer = term.buffer.active;
  const start = Math.max(0, buffer.length - TERMINAL_TAIL_LINES);
  const lines: string[] = [];
  for (let index = start; index < buffer.length; index += 1) {
    lines.push(buffer.getLine(index)?.translateToString(true) ?? "");
  }
  return lastLines(lines.join("\n"));
}

/** The whole scrollback as plain text. Soft-wrapped rows are joined, so a long path stays whole. */
export function terminalScrollbackText(id: string): string {
  const term = parked.get(id)?.term;
  if (!term) return "";
  const buffer = term.buffer.active;
  const lines: string[] = [];
  for (let index = 0; index < buffer.length; index += 1) {
    const line = buffer.getLine(index);
    const text = line?.translateToString(true) ?? "";
    if (line?.isWrapped && lines.length > 0) lines[lines.length - 1] += text;
    else lines.push(text);
  }
  return lines.join("\n");
}

/** Clear the screen and the scrollback; the current prompt line stays. */
export function clearTerminalScrollback(id: string): void {
  parked.get(id)?.term.clear();
}

/** Paste text as the user's own paste: bracketed when the program asked for it. */
export function pasteIntoTerminal(id: string, text: string): void {
  if (text) parked.get(id)?.term.paste(text);
}

/** A paste from the app (the scratchpad, the context menu): a big one is offered for compaction first. */
export function pasteWithOffer(id: string, text: string): void {
  const term = parked.get(id)?.term;
  if (text && term) pasteOrOffer(id, text, compactPasteOn, (t) => term.paste(t));
}

export async function copyTerminalSelection(id: string): Promise<void> {
  const text = parked.get(id)?.term.getSelection() ?? "";
  if (!text) return;
  await navigator.clipboard.writeText(text);
}

export async function pasteTerminalText(id: string): Promise<void> {
  const text = await navigator.clipboard.readText();
  if (!text) return;
  pasteWithOffer(id, text);
}
