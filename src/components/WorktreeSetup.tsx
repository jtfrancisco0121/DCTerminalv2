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
  const [liveTests, setLiveTests] = useState(repo.liveTests ?? false);
  const [receipt, setReceipt] = useState(repo.receipt ?? false);
  const [qaOn, setQaOn] = useState(Boolean(repo.qa));
  const [qaCommand, setQaCommand] = useState(repo.qa?.command ?? "");
  const [qaUrl, setQaUrl] = useState(repo.qa?.url ?? "http://127.0.0.1:%PORT%/");
  const [note, setNote] = useState<{ tone: "hint" | "error"; text: string } | null>(null);
  const errorId = `qa-error-${repo.name}`;
  const save = async () => {
    setNote(null);
    const next: RepoSetup = {
      setup: setup.trim() || null,
      copyFiles: parseCopyFiles(files),
      liveTests,
      receipt,
      qa: qaOn ? { command: qaCommand, url: qaUrl } : null,
    };
    try {
      const saved = await setRepoSetup(repo.path, next);
      setSetup(saved.setup ?? "");
      setFiles(saved.copyFiles.join("\n"));
      setLiveTests(saved.liveTests ?? false);
      setReceipt(saved.receipt ?? false);
      setQaOn(Boolean(saved.qa));
      if (saved.qa) {
        setQaCommand(saved.qa.command);
        setQaUrl(saved.qa.url);
      }
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
      <label className="check-label">
        <input
          type="checkbox"
          aria-label={`Receipts in PR bodies for ${repo.name}`}
          checked={receipt}
          onChange={(event) => setReceipt(event.target.checked)}
        />{" "}
        Receipts in PR bodies (a collapsed section with agents, minutes, tokens and the five-hour window share; a
        public PR body shows this usage)
      </label>
      <label className="check-label">
        <input
          type="checkbox"
          aria-label={`Try PRs in the app (QA) for ${repo.name}`}
          checked={qaOn}
          onChange={(event) => setQaOn(event.target.checked)}
        />{" "}
        Try PRs in the app (QA): after a clean review, a headless browser tries the plan&apos;s in-app criteria on a
        local copy of the PR. A change applies to the next run.
      </label>
      {qaOn && (
        <>
          <label className="field-label">
            QA start command, run in the PR&apos;s worktree (use %PORT% or the PORT environment variable)
            <input
              className="text-input"
              aria-label={`QA command of ${repo.name}`}
              aria-describedby={note?.tone === "error" ? errorId : undefined}
              value={qaCommand}
              placeholder="npm run dev -- --port %PORT%"
              onChange={(event) => setQaCommand(event.target.value)}
            />
          </label>
          <label className="field-label">
            QA URL (http, on 127.0.0.1, localhost or [::1]; %PORT% is the free port)
            <input
              className="text-input"
              aria-label={`QA URL of ${repo.name}`}
              aria-describedby={note?.tone === "error" ? errorId : undefined}
              value={qaUrl}
              onChange={(event) => setQaUrl(event.target.value)}
            />
          </label>
        </>
      )}
      <div className="button-row">
        <button type="button" className="secondary-button" onClick={() => void save()}>
          Save
        </button>
        {note && (
          <span
            id={note.tone === "error" ? errorId : undefined}
            className={note.tone}
            role={note.tone === "error" ? "alert" : undefined}
          >
            {note.text}
          </span>
        )}
      </div>
    </li>
  );
}
