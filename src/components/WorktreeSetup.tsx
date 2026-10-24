import { useState } from "react";
import { setRepoSetup, type Repo, type RepoSetup } from "../run/api";
import { errorText } from "./InlineNotice";

/** One path per line (or comma-separated), blanks dropped. */
export function parseCopyFiles(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((line) => line.trim())
    .filter(Boolean);
}

/**
 * Start screen › Worktree setup: per repo of the start folder, the setup
 * command run in a new run worktree (empty: detected from the lockfile)
 * and the local files copied into it. Off / empty by default.
 */
export function WorktreeSetup({ repos, onSaved }: { repos: Repo[]; onSaved?: () => void }) {
  return (
    <details className="repo-setup" data-testid="worktree-setup">
      <summary>Worktree setup</summary>
      <p className="hint">
        Each run worktree gets its packages installed from the repo&apos;s lockfile. Override the command here, and
        list local files to copy from the main checkout (for example <code>.env</code>). Only git-ignored files are
        copied, never committed, and removed with the worktree.
      </p>
      <p className="hint warn">Files you list here may hold secrets: they are copied into every run worktree of that repo.</p>
      <ul>
        {repos.map((repo) => (
          <RepoRow key={repo.path} repo={repo} onSaved={onSaved} />
        ))}
      </ul>
    </details>
  );
}

function RepoRow({ repo, onSaved }: { repo: Repo; onSaved?: () => void }) {
  const [setup, setSetup] = useState(repo.setup ?? "");
  const [files, setFiles] = useState((repo.copyFiles ?? []).join("\n"));
  const [note, setNote] = useState<{ tone: "hint" | "error"; text: string } | null>(null);
  const save = async () => {
    setNote(null);
    const next: RepoSetup = { setup: setup.trim() || null, copyFiles: parseCopyFiles(files) };
    try {
      const saved = await setRepoSetup(repo.path, next);
      setSetup(saved.setup ?? "");
      setFiles(saved.copyFiles.join("\n"));
      setNote({ tone: "hint", text: "Saved." });
      onSaved?.();
    } catch (err) {
      setNote({ tone: "error", text: errorText(err) });
    }
  };
  return (
    <li className="account-row">
      <strong>{repo.name}</strong>
      <label className="field-label">
        Setup command (empty: from the lockfile)
        <input
          className="text-input"
          aria-label={`Setup command of ${repo.name}`}
          value={setup}
          placeholder="npm ci"
          onChange={(event) => setSetup(event.target.value)}
        />
      </label>
      <label className="field-label">
        Copy these local files into run worktrees (one per line)
        <textarea
          className="text-input"
          aria-label={`Files to copy for ${repo.name}`}
          rows={2}
          value={files}
          placeholder=".env.local"
          onChange={(event) => setFiles(event.target.value)}
        />
      </label>
      <div className="button-row">
        <button type="button" className="secondary-button" onClick={() => void save()}>
          Save
        </button>
        {note && <span className={note.tone}>{note.text}</span>}
      </div>
    </li>
  );
}
