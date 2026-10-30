/**
 * Compact paste: a big paste (a build or test log) is offered in a bar with
 * its size before and after the backend's `compact_text`, and nothing is
 * changed unless the user picks "Paste compacted".
 */
import { invoke } from "@tauri-apps/api/core";

/** A paste is offered only above this many lines or this many bytes. */
export const OFFER_LINES = 200;
export const OFFER_BYTES = 16 * 1024;

export type Compacted = { text: string; linesBefore: number; linesAfter: number };

/** Estimated tokens: characters / 4, rounded up. */
export const estimateTokens = (text: string): number => Math.ceil(text.length / 4);

export function lineCount(text: string): number {
  if (!text) return 0;
  let n = 1;
  for (let i = 0; i < text.length; i += 1) if (text.charCodeAt(i) === 10) n += 1;
  return text.endsWith("\n") ? n - 1 : n;
}

/** Whether the bar is offered for this text (the setting must also be on). */
export function shouldOffer(text: string, enabled: boolean): boolean {
  if (!enabled) return false;
  return new TextEncoder().encode(text).length > OFFER_BYTES || lineCount(text) > OFFER_LINES;
}

export const compactText = (text: string) => invoke<Compacted>("compact_text", { text });

export type PasteOffer = { id: string; text: string; paste: (text: string) => void };
type Listener = (offer: PasteOffer) => void;
const listeners = new Set<Listener>();

/** The PasteBar of the tab subscribes here. */
export function onPasteOffer(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/**
 * Pastes `text` into the terminal `id` through `paste`; a big paste with the
 * offer on goes to the bar instead (and is pasted as is when no bar is
 * mounted). Returns true when the bar took it.
 */
export function pasteOrOffer(id: string, text: string, enabled: boolean, paste: (text: string) => void): boolean {
  if (!shouldOffer(text, enabled) || listeners.size === 0) {
    paste(text);
    return false;
  }
  for (const listener of listeners) listener({ id, text, paste });
  return true;
}
