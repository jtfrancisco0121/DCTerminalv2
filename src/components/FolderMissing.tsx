import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { setTabFolder } from "../bridge";
import { errorText } from "./InlineNotice";

type Props = {
  tabId: string;
  /** Why the folder does not work (from `folderProblem`). */
  problem: string;
  /** The tab now points at another folder: the window reads its tabs again. */
  onChanged: () => void;
};

/** A tab whose folder is gone: pick another folder and the tab starts there. */
export function FolderMissing({ tabId, problem, onChanged }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const choose = async () => {
    setError(null);
    try {
      const picked = await open({ directory: true, multiple: false });
      if (typeof picked !== "string") return;
      setBusy(true);
      await setTabFolder(tabId, picked);
      onChanged();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="terminal-view terminal-problem">
      <p className="error">{problem}</p>
      <p className="hint">Point this tab at another folder, or close it and open a new one.</p>
      <div className="button-row">
        <button type="button" className="primary-button" disabled={busy} onClick={() => void choose()}>
          Choose another folder…
        </button>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
