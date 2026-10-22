/**
 * Errors the interface caught, newest last: failed calls passed to
 * reportError. Kept in memory for this window only; `notify` ones also show
 * as a toast (App sets the notifier).
 */

import { errorText } from "./components/InlineNotice";

export type UiError = { at: number; source: string; message: string };

const MAX_ERRORS = 50;

let errors: readonly UiError[] = [];
const listeners = new Set<() => void>();

export function recordUiError(source: string, error: unknown): void {
  errors = [...errors, { at: Date.now(), source, message: errorText(error) }].slice(-MAX_ERRORS);
  for (const listener of listeners) listener();
}

/** A stable array until the next error (fit for useSyncExternalStore). */
export function uiErrors(): readonly UiError[] {
  return errors;
}

export function subscribeUiErrors(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

type Notifier = (title: string, message: string) => void;
let notifier: Notifier | null = null;
/** `once` keys already reported in this window. */
const reported = new Set<string>();

/** Where reportError shows a toast: App's error toast. */
export function setErrorNotifier(next: Notifier | null): void {
  notifier = next;
}

export type ReportOptions = {
  /** Also show a "failed" toast titled `source`: a user action, or a failure nothing else shows. */
  notify?: boolean;
  /**
   * Report only the first failure in this window: a background call that
   * retries, or a stream such as keystrokes. True keys on `source`; a string
   * keys on itself, so several calls can share one report.
   */
  once?: boolean | string;
};

/**
 * The one way a caught error is reported instead of being swallowed: always
 * to the in-memory list, and with `notify` as a toast too.
 */
export function reportError(source: string, error: unknown, options: ReportOptions = {}): void {
  const key = options.once === true ? source : options.once || null;
  if (key !== null) {
    if (reported.has(key)) return;
    reported.add(key);
  }
  recordUiError(source, error);
  if (options.notify) notifier?.(source, errorText(error));
}

/** For tests. */
export function clearUiErrors(): void {
  errors = [];
  reported.clear();
  for (const listener of listeners) listener();
}
