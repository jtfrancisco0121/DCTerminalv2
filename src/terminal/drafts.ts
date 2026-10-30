/**
 * The scratchpad's drafts: one per pane and the last 50 sent prompts, kept in
 * `<app data>/drafts.json` through `drafts_load` / `drafts_save`. A change is
 * saved 300 ms after the last keystroke, and at once on `flush` (blur, close).
 */
import { invoke } from "@tauri-apps/api/core";

export const SAVE_DELAY_MS = 300;
export const HISTORY_MAX = 50;

export type DraftsFile = { drafts: Record<string, string>; history: string[] };

export const loadDrafts = () => invoke<DraftsFile>("drafts_load");
const saveDrafts = (file: DraftsFile) => invoke<void>("drafts_save", { drafts: file.drafts, history: file.history });

export class DraftStore {
  private file: DraftsFile = { drafts: {}, history: [] };
  private timer: number | undefined;
  private dirty = false;
  private ready: Promise<void>;

  constructor(
    private readonly onError: (err: unknown) => void = () => undefined,
    load: () => Promise<DraftsFile> = loadDrafts,
  ) {
    this.ready = load().then(
      (file) => {
        // Edits made while loading win over the file.
        this.file = {
          drafts: { ...file.drafts, ...this.file.drafts },
          history: this.file.history.length ? this.file.history : file.history,
        };
      },
      (err: unknown) => this.onError(err),
    );
  }

  whenReady(): Promise<void> {
    return this.ready;
  }

  get(pane: string): string {
    return this.file.drafts[pane] ?? "";
  }

  history(): readonly string[] {
    return this.file.history;
  }

  /** The draft changed: saved after the debounce. */
  set(pane: string, text: string): void {
    this.file.drafts[pane] = text;
    this.schedule();
  }

  /** A prompt was sent: it goes to the history (last 50, no repeat of the newest) and the pane's draft is cleared. */
  sent(pane: string, text: string): void {
    const last = this.file.history[this.file.history.length - 1];
    if (text && text !== last) this.file.history = [...this.file.history, text].slice(-HISTORY_MAX);
    this.file.drafts[pane] = "";
    this.flush();
  }

  private schedule(): void {
    this.dirty = true;
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.flush(), SAVE_DELAY_MS);
  }

  /** Saves now when something changed (blur, closing the pad, leaving the page). */
  flush(): void {
    window.clearTimeout(this.timer);
    this.timer = undefined;
    if (!this.dirty && this.file.history.length === 0) return;
    this.dirty = false;
    void this.ready.then(() => saveDrafts(this.file).catch(this.onError));
  }
}

let shared: DraftStore | null = null;

/** The one store of this page. */
export function draftStore(onError?: (err: unknown) => void): DraftStore {
  shared ??= new DraftStore(onError);
  return shared;
}
