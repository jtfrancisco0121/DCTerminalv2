import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { TabKind } from "../bridge";
import type { NewTabDraft } from "../newTab";
import { errorText } from "./InlineNotice";

type Props = {
  initial: NewTabDraft;
  /** Creates the tab; a rejection (a folder that does not exist) is shown here. */
  onCreate: (draft: NewTabDraft) => Promise<void>;
  onCancel?: () => void;
};

/** Folder and kind for a new tab. Used in the new-tab dialog and as an empty window's start screen. */
export function NewTabForm({ initial, onCreate, onCancel }: Props) {
  const [cwd, setCwd] = useState(initial.cwd);
  const [kind, setKind] = useState<TabKind>(initial.kind);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const choose = async () => {
    try {
      const picked = await open({ directory: true, multiple: false, defaultPath: cwd || undefined });
      if (typeof picked === "string") setCwd(picked);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const submit = async () => {
    if (!cwd.trim()) {
      setError("Choose a folder.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onCreate({ cwd: cwd.trim(), kind });
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="new-tab-form"
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <label className="field-label">
        Folder
        <span className="field-row">
          <input
            className="text-input"
            value={cwd}
            placeholder="/path/to/project"
            onChange={(event) => setCwd(event.target.value)}
          />
          <button type="button" className="secondary-button" onClick={() => void choose()}>
            Choose…
          </button>
        </span>
      </label>
      <fieldset className="kind-choice">
        <legend>Runs</legend>
        <label>
          <input type="radio" name="kind" checked={kind === "claude"} onChange={() => setKind("claude")} /> Claude
        </label>
        <label>
          <input type="radio" name="kind" checked={kind === "shell"} onChange={() => setKind("shell")} /> Shell
        </label>
      </fieldset>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="button-row">
        <button type="submit" className="primary-button" disabled={busy}>
          Open tab
        </button>
        {onCancel && (
          <button type="button" className="secondary-button" onClick={onCancel}>
            Cancel
          </button>
        )}
      </div>
    </form>
  );
}
