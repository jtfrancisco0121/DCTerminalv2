import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";

export type ImportStatus = { available: boolean; offered: boolean; importedAt: string | null; devDir: string };
export type ImportSummary = {
  windows: string[];
  tabs: number;
  closedTabs: number;
  runs: number;
  accountsAdded: string[];
  memoryCopied: boolean;
  notes: string[];
};

export const devImportStatus = () => invoke<ImportStatus>("dev_import_status");
export const devImportDismiss = () => invoke<void>("dev_import_dismiss");
export const importFromDevApp = () => invoke<ImportSummary>("import_from_dev_app");

export function summaryText(s: ImportSummary): string {
  const parts = [
    `${s.tabs} ${s.tabs === 1 ? "tab" : "tabs"} in ${s.windows.length} ${s.windows.length === 1 ? "window" : "windows"}`,
    `${s.closedTabs} recently closed`,
    `${s.runs} ${s.runs === 1 ? "run" : "runs"} adopted in place`,
  ];
  if (s.memoryCopied) parts.push("memory copied");
  if (s.accountsAdded.length > 0) parts.push(`accounts added: ${s.accountsAdded.join(", ")}`);
  return `Imported ${parts.join(", ")}.`;
}

/**
 * Import from the dev app (release builds): the dev app's tabs (resumed as
 * they were), windows and accounts, its runs (used in place) and memory.
 * Nothing in the dev app's folder is changed.
 */
export function ImportDevDialog({ status, onClose }: { status: ImportStatus; onClose: () => void }) {
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const later = () => {
    void devImportDismiss().catch(() => undefined);
    onClose();
  };
  const run = async () => {
    setBusy(true);
    setProblem(null);
    try {
      const summary = await importFromDevApp();
      setDone([summaryText(summary), ...summary.notes].join(" "));
    } catch (err) {
      setProblem(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog title="Import from the dev app" onClose={done ? onClose : later}>
      {!status.available && <p className="hint">No dev app data was found at {status.devDir}.</p>}
      {status.available && !done && (
        <>
          <p>
            Bring the dev app&apos;s tabs, windows, Eagle-Eye runs and memory into this app. Each tab resumes its Claude
            conversation in the same folder and account; runs keep working from their folders in the dev app&apos;s data.
          </p>
          <p className="hint">
            Nothing in {status.devDir} is changed. macOS may ask once to let this app use the keychain items (ClickUp
            tokens, the signing key) the dev app saved.
          </p>
          {status.importedAt && <p className="hint">Imported before at {status.importedAt}; only new tabs and runs come in.</p>}
        </>
      )}
      {done && (
        <p role="status" data-testid="import-done">
          {done}
        </p>
      )}
      {problem && (
        <p className="error" role="alert">
          {problem}
        </p>
      )}
      <div className="button-row">
        {status.available && !done && (
          <button type="button" className="primary-button" disabled={busy} onClick={() => void run()}>
            {busy ? "Importing…" : "Import"}
          </button>
        )}
        <button type="button" className="secondary-button" onClick={done ? onClose : later}>
          {done ? "Close" : "Not now"}
        </button>
      </div>
    </Dialog>
  );
}

/** Offers the import once on first launch, and on File › Import from dev app…. */
export function useDevImport(windowId: string): {
  status: ImportStatus | null;
  open: () => void;
  close: () => void;
} {
  const [status, setStatus] = useState<ImportStatus | null>(null);
  useEffect(() => {
    if (windowId !== "main") return;
    devImportStatus().then(
      (next) => {
        if (next?.available && !next.offered) setStatus(next);
      },
      () => undefined,
    );
  }, [windowId]);
  return {
    status,
    open: () => void devImportStatus().then(setStatus, () => undefined),
    close: () => setStatus(null),
  };
}
