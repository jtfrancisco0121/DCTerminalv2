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

const LIMIT_ERROR = "Use a whole number from 1 to 100000.";

/** The PR line limit field: empty is no limit set (null), else a whole number 1..100000, or undefined when invalid. */
export function parseLimit(text: string): number | null | undefined {
  const t = text.trim();
  if (!t) return null;
  if (!/^\d+$/.test(t)) return undefined;
  const n = Number(t);
  return n >= 1 && n <= 100000 ? n : undefined;
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
  const [liveTests, setLiveTests] = useState(repo.liveTests ?? false);
  const [limit, setLimit] = useState(repo.prSizeLimit == null ? "" : String(repo.prSizeLimit));
  const [note, setNote] = useState<{ tone: "hint" | "error"; text: string } | null>(null);
  const save = async () => {
    setNote(null);
    const prSizeLimit = parseLimit(limit);
    if (prSizeLimit === undefined) {
      setNote({ tone: "error", text: LIMIT_ERROR });
      return;
    }
    const next: RepoSetup = { setup: setup.trim() || null, copyFiles: parseCopyFiles(files), liveTests, prSizeLimit };
    try {
      const saved = await setRepoSetup(repo.path, next);
      setSetup(saved.setup ?? "");
      setFiles(saved.copyFiles.join("\n"));
      setLiveTests(saved.liveTests ?? false);
      setLimit(saved.prSizeLimit == null ? "" : String(saved.prSizeLimit));
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
      <label className="field-label">
        PR line limit (empty: 400)
        <input
          className="text-input"
          aria-label={`PR line limit of ${repo.name}`}
          inputMode="numeric"
          value={limit}
          placeholder="400"
          onChange={(event) => setLimit(event.target.value)}
        />
      </label>
      <label className="check-label">
        <input
          type="checkbox"
          aria-label={`Allow live Claude tests in checks for ${repo.name}`}
          checked={liveTests}
          onChange={(event) => setLiveTests(event.target.checked)}
        />{" "}
        Allow live Claude tests in checks (the Implementer and PR Reviewer may run{" "}
        <code>scripts/skill-e2e/golden.sh</code>; they use your Claude usage)
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
