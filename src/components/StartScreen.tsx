import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  detectFolderRepos,
  resumeRun,
  runOverview,
  startRun,
  type FolderRepos,
  type RunOverview,
  type StartedRun,
} from "../run/api";
import { stageText } from "../run/words";
import { errorText } from "./InlineNotice";
import { MemoryPanel } from "./MemoryPanel";
import { ClickUpIntake } from "../clickup/ClickUpIntake";
import type { ClickUpStart } from "../clickup/api";

type Props = {
  windowId: string;
  initialFolder: string;
  onStarted: (started: StartedRun) => void;
  /** Open a plain Claude or shell tab instead. */
  onPlainTab: () => void;
};

function reposText(found: FolderRepos | null): string {
  if (!found) return "";
  switch (found.kind) {
    case "repo":
      return "A git repo.";
    case "workspace":
      return `A folder of ${found.repos.length} git ${found.repos.length === 1 ? "repo" : "repos"}.`;
    case "inside-repo":
      return "This is inside a git repo. Pick the repo's top folder for Eagle-Eye.";
    default:
      return "No git repo here. Eagle-Eye needs a git repo or a folder of git repos.";
  }
}

/**
 * New run: a folder, what to do, and Start Eagle-Eye. One role on its own
 * is one tile. Earlier runs can be resumed.
 */
export function StartScreen({ windowId, initialFolder, onStarted, onPlainTab }: Props) {
  const [folder, setFolder] = useState(initialFolder);
  const [found, setFound] = useState<FolderRepos | null>(null);
  const [request, setRequest] = useState("");
  const [overview, setOverview] = useState<RunOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [tickets, setTickets] = useState<ClickUpStart | null>(null);

  useEffect(() => {
    runOverview().then(setOverview, (err: unknown) => setError(errorText(err)));
  }, []);

  useEffect(() => {
    setFound(null);
    const trimmed = folder.trim();
    if (!trimmed) return;
    let current = true;
    const timer = window.setTimeout(() => {
      detectFolderRepos(trimmed).then(
        (next) => current && setFound(next),
        () => current && setFound({ kind: "none", repos: [] }),
      );
    }, 250);
    return () => {
      current = false;
      window.clearTimeout(timer);
    };
  }, [folder]);

  const choose = async () => {
    try {
      const picked = await open({ directory: true, multiple: false, defaultPath: folder || undefined });
      if (typeof picked === "string") setFolder(picked);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const launch = async (role: string) => {
    if (!folder.trim()) {
      setError("Choose a folder.");
      return;
    }
    const eagleTickets = role === "eagle-eye" ? tickets : null;
    if (role === "eagle-eye" && !request.trim() && !eagleTickets) {
      setError("Say what should be done.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const said = request.trim() || (eagleTickets ? "Do the ClickUp tickets below." : request);
      onStarted(await startRun(windowId, folder.trim(), said, role, eagleTickets));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const resume = async (runDir: string) => {
    setBusy(true);
    setError(null);
    try {
      onStarted(await resumeRun(windowId, runDir));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const canEagle = found !== null && found.repos.length > 0;
  const runs = (overview?.runs ?? []).slice(0, 8);

  return (
    <div className="run-start">
      <label className="field-label">
        Folder
        <span className="field-row">
          <input
            className="text-input"
            value={folder}
            placeholder="/path/to/repo or folder of repos"
            onChange={(event) => setFolder(event.target.value)}
          />
          <button type="button" className="secondary-button" onClick={() => void choose()}>
            Choose…
          </button>
        </span>
      </label>
      {found && (
        <div className="run-repos" data-testid="repos">
          <p className="hint">{reposText(found)}</p>
          {found.repos.length > 0 && (
            <ul>
              {found.repos.map((repo) => (
                <li key={repo.path}>
                  <strong>{repo.name}</strong>
                  <span className="hint"> — check: {repo.check ?? "none known"}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      <label className="field-label">
        What should be done?
        <textarea
          className="text-input run-request"
          rows={5}
          value={request}
          placeholder="Describe the feature or the bug, in a few lines."
          onChange={(event) => setRequest(event.target.value)}
        />
      </label>
      <ClickUpIntake windowId={windowId} onChange={setTickets} disabled={busy} />
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {overview && !overview.pluginDir && <p className="hint warn">The dcv2 plugin was not found.</p>}
      <div className="button-row">
        <button
          type="button"
          className="primary-button"
          disabled={busy || !canEagle}
          onClick={() => void launch("eagle-eye")}
        >
          Start Eagle-Eye
        </button>
        <button type="button" className="link-button" onClick={onPlainTab}>
          Plain terminal tab instead
        </button>
      </div>
      <p className="hint run-tiles-hint">Or one role on its own, in this folder:</p>
      <div className="role-tiles">
        {(overview?.roles ?? []).map((role) => (
          <button
            key={role.id}
            type="button"
            className="role-tile"
            disabled={busy || !folder.trim()}
            onClick={() => void launch(role.id)}
          >
            {role.label}
          </button>
        ))}
      </div>
      {folder.trim() && found && found.repos.length > 0 && (
        <MemoryPanel folder={folder.trim()} onOnboard={() => void launch("onboard")} />
      )}
      {runs.length > 0 && (
        <section className="run-list" aria-label="Earlier runs">
          <h3>Earlier runs</h3>
          <ul>
            {runs.map((run) => (
              <li key={run.runDir}>
                <span className="run-list-title">{run.title ?? run.runId}</span>
                <span className="hint">
                  {" "}
                  — {stageText(run.stage)}
                  {run.folder ? ` · ${run.folder}` : ""}
                </span>
                {run.stage !== "done" && (
                  <button
                    type="button"
                    className="link-button"
                    disabled={busy}
                    onClick={() => void resume(run.runDir)}
                  >
                    Resume
                  </button>
                )}
              </li>
            ))}
          </ul>
        </section>
      )}
      {overview?.pluginVersion && <p className="hint run-version">dcv2 roles {overview.pluginVersion}</p>}
    </div>
  );
}
