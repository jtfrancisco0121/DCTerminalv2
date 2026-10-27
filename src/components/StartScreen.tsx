import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import {
  detectFolderRepos,
  duplicateOf,
  readRunFile,
  resumeRun,
  runOverview,
  startRun,
  type DuplicateRun,
  type FolderRepos,
  type BmadMode,
  type ReviewMode,
  type RunOverview,
  type StartedRun,
} from "../run/api";
import { WorktreeSetup } from "./WorktreeSetup";
import { estimate, estimateText } from "../run/estimate";
import { relativeDate, stageText } from "../run/words";
import { TextDialog } from "./RunPanel";
import { Dialog } from "./Dialog";
import { RecentFolderOptions, useRecentFolders } from "./RecentFolders";
import { nearLimitWarning, type RateWindow } from "../usage/limits";
import { getSettings } from "../bridge";
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
  /** Open the Runs view (every run, with search). */
  onShowAllRuns?: () => void;
  /** Claude's limit windows, for the near-limit warning (never blocks starting). */
  usageWindows?: RateWindow[] | null;
};

/** How many earlier runs the start screen lists; the Runs view has them all. */
export const EARLIER_RUNS = 8;

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
 * Why Start Eagle-Eye is disabled, or null when it can start: the first
 * matching reason, in the order of the UX voice table.
 */
export function disabledReason(input: {
  folder: string;
  found: FolderRepos | null;
  hasRequest: boolean;
  hasTickets: boolean;
  noRepoTicked?: boolean;
  tooMany: string | null;
  busy: boolean;
}): string | null {
  if (!input.folder.trim()) return "Choose a folder first.";
  if (!input.found) return "Looking at the folder…";
  if (input.found.kind === "inside-repo") return "Pick the repo's top folder.";
  if (input.found.repos.length === 0) return "This folder has no git repo. Pick a repo or a folder of repos.";
  if (!input.hasRequest && !input.hasTickets) return "Say what should be done, or fetch ClickUp tickets.";
  if (input.noRepoTicked) return "Tick at least one repo.";
  if (input.tooMany) return input.tooMany;
  if (input.busy) return "Starting…";
  return null;
}

/** The implementer count field's value as 1..=8; an empty or unreadable one is `fallback`. */
export function clampImplementers(text: string, fallback: number): number {
  const n = Math.round(Number(text));
  if (!text.trim() || !Number.isFinite(n)) return Math.min(8, Math.max(1, fallback));
  return Math.min(8, Math.max(1, n));
}

const REVIEW_KEY = "dcv2.reviewOnGitHub.";

/** The last "Review on GitHub" choice of this window; on by default. */
export function rememberedReviewOnGitHub(windowId: string): boolean {
  try {
    return localStorage.getItem(REVIEW_KEY + windowId) !== "off";
  } catch {
    return true;
  }
}

function rememberReviewOnGitHub(windowId: string, on: boolean): void {
  try {
    localStorage.setItem(REVIEW_KEY + windowId, on ? "on" : "off");
  } catch {
    // No storage: the default (on) next time.
  }
}

const BMAD_KEY = "dcv2.bmadMode.";
const BMAD_MODES: BmadMode[] = ["auto", "on", "off"];
const BMAD_LABELS: Record<BmadMode, string> = { auto: "Auto", on: "On", off: "Off" };

/** The last BMAD choice for this start folder; Auto by default (also with no storage or an unknown value). */
export function rememberedBmadMode(folder: string): BmadMode {
  try {
    const stored = localStorage.getItem(BMAD_KEY + folder);
    return BMAD_MODES.find((m) => m === stored) ?? "auto";
  } catch {
    return "auto";
  }
}

function rememberBmadMode(folder: string, mode: BmadMode): void {
  try {
    localStorage.setItem(BMAD_KEY + folder, mode);
  } catch {
    // No storage: Auto next time.
  }
}

/**
 * New run: a folder, what to do, and Start Eagle-Eye. One role on its own
 * is one tile. Earlier runs can be resumed.
 */
export function StartScreen({ windowId, initialFolder, onStarted, onPlainTab, onShowAllRuns, usageWindows }: Props) {
  const [folder, setFolder] = useState(initialFolder);
  const [found, setFound] = useState<FolderRepos | null>(null);
  const [request, setRequest] = useState("");
  const [overview, setOverview] = useState<RunOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const recent = useRecentFolders();
  const [report, setReport] = useState<{ title: string; text: string } | null>(null);
  // The same unfinished request is already running: ask before starting it again.
  const [duplicate, setDuplicate] = useState<{ run: DuplicateRun; role: string } | null>(null);
  const [tickets, setTickets] = useState<ClickUpStart | null>(null);
  const [tooMany, setTooMany] = useState<string | null>(null);
  // The repos ticked for this run (all, until you untick; reset with the folder), and the implementer count.
  const [unticked, setUnticked] = useState<ReadonlySet<string>>(new Set());
  const [defaultImplementers, setDefaultImplementers] = useState(3);
  const [implementers, setImplementers] = useState<string | null>(null);
  const [reviewOnGitHub, setReviewOnGitHub] = useState(() => rememberedReviewOnGitHub(windowId));
  const [bmadMode, setBmadMode] = useState<BmadMode>(() => rememberedBmadMode(initialFolder.trim()));

  // BMAD is remembered per start folder (the design says per repo), unlike the
  // review box, which is per window.
  useEffect(() => {
    setBmadMode(rememberedBmadMode(folder.trim()));
  }, [folder]);

  useEffect(() => {
    getSettings().then(
      (settings) => setDefaultImplementers(settings.run?.maxImplementers ?? 3),
      () => undefined, // The default stays 3; Settings reports its own failures.
    );
  }, []);

  useEffect(() => {
    setUnticked(new Set());
  }, [folder, found]);

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

  const launch = async (role: string, force = false) => {
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
      const mode: ReviewMode | null = role === "eagle-eye" ? (reviewOnGitHub ? "github" : "local") : null;
      const bmad: BmadMode | null = role === "eagle-eye" ? bmadMode : null;
      const eagle = role === "eagle-eye";
      onStarted(
        await startRun(
          windowId,
          folder.trim(),
          said,
          role,
          eagleTickets,
          mode,
          bmad,
          force,
          eagle ? tickedRepos : null,
          eagle && implementers !== null ? implementersValue : null,
        ),
      );
    } catch (err) {
      const same = duplicateOf(err);
      if (same) setDuplicate({ run: same, role });
      else setError(errorText(err));
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

  const showReport = (run: { runDir: string; title: string | null; runId: string }) =>
    readRunFile(run.runDir, "report.md").then(
      (text) => setReport({ title: `Report: ${run.title ?? run.runId}`, text }),
      (err: unknown) => setError(errorText(err)),
    );

  const choosable = (found?.repos.length ?? 0) > 1;
  const tickedRepos = choosable ? (found?.repos ?? []).filter((r) => !unticked.has(r.name)).map((r) => r.name) : null;
  const implementersValue = clampImplementers(implementers ?? String(defaultImplementers), defaultImplementers);
  // The estimate never affects `disabled`: it only informs.
  const costEstimate =
    overview && found && found.repos.length > 0
      ? estimate(
          overview.runs,
          tickedRepos ? tickedRepos.length : found.repos.length,
          bmadMode === "auto" ? null : bmadMode === "on",
        )
      : undefined;
  const startReason = disabledReason({
    folder,
    found,
    hasRequest: Boolean(request.trim()),
    hasTickets: Boolean(tickets),
    noRepoTicked: tickedRepos !== null && tickedRepos.length === 0,
    tooMany,
    busy,
  });
  const runs = (overview?.runs ?? []).slice(0, EARLIER_RUNS);

  return (
    <div className="run-start">
      <label className="field-label">
        Folder
        <span className="field-row">
          <input
            className="text-input"
            value={folder}
            list="start-recent-folders"
            placeholder="/path/to/repo or folder of repos"
            onChange={(event) => setFolder(event.target.value)}
          />
          <RecentFolderOptions id="start-recent-folders" folders={recent} />
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
                  {choosable ? (
                    <label className="check-label run-repo-pick">
                      <input
                        type="checkbox"
                        checked={!unticked.has(repo.name)}
                        disabled={busy}
                        onChange={(event) =>
                          setUnticked((current) => {
                            const next = new Set(current);
                            if (event.target.checked) next.delete(repo.name);
                            else next.add(repo.name);
                            return next;
                          })
                        }
                      />
                      <strong>{repo.name}</strong>
                    </label>
                  ) : (
                    <strong>{repo.name}</strong>
                  )}
                  <span className="hint"> — check: {repo.check ?? "none known"}</span>
                </li>
              ))}
            </ul>
          )}
          {found.repos.length > 0 && (
            <label className="field-label run-implementers">
              Implementers
              <input
                className="text-input run-implementers-input"
                type="number"
                min={1}
                max={8}
                value={implementers ?? String(defaultImplementers)}
                disabled={busy}
                aria-label="Implementers"
                onChange={(event) => setImplementers(event.target.value)}
                onBlur={() => implementers !== null && setImplementers(String(implementersValue))}
              />
              <span className="hint"> at most this many PRs build at once in this run (Settings has the default)</span>
            </label>
          )}
          {found.repos.length > 0 && (
            <WorktreeSetup key={found.repos.map((r) => r.path).join("|")} repos={found.repos} />
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
      <ClickUpIntake windowId={windowId} onChange={setTickets} onTooMany={setTooMany} disabled={busy} />
      <label className="check-label">
        <input
          type="checkbox"
          checked={reviewOnGitHub}
          onChange={(event) => {
            setReviewOnGitHub(event.target.checked);
            rememberReviewOnGitHub(windowId, event.target.checked);
          }}
        />
        Review on GitHub (draft PRs)
      </label>
      <p className="hint">
        {reviewOnGitHub
          ? "After the first build each PR opens as a draft on GitHub, every review round is posted as a PR comment, and the drafts are marked ready at the end. Teammates can see them."
          : "Reviews stay in the run folder; PRs open only at the end."}
      </p>
      <fieldset className="bmad-mode" aria-label="BMAD planning">
        <legend>BMAD planning (BMAD-METHOD skills)</legend>
        {BMAD_MODES.map((mode) => (
          <label key={mode} className="check-label bmad-choice">
            <input
              type="radio"
              name="bmad-mode"
              value={mode}
              checked={bmadMode === mode}
              onChange={() => {
                setBmadMode(mode);
                rememberBmadMode(folder.trim(), mode);
              }}
            />
            {BMAD_LABELS[mode]}
          </label>
        ))}
      </fieldset>
      <p className="hint">
        {bmadMode === "auto"
          ? "Large work (several repos, a new API or screen, many stories) gets a spec and an architecture written before the plan. Small work skips it."
          : bmadMode === "on"
            ? "Every run gets a spec and an architecture written before the plan. Needs Node 18 or newer."
            : "No BMAD documents; the plan is written straight from the request."}
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {nearLimitWarning(usageWindows) && (
        <p className="hint warn" role="status" data-testid="near-limit">
          {nearLimitWarning(usageWindows)}
        </p>
      )}
      {overview && !overview.pluginDir && <p className="hint warn">The dcv2 plugin was not found.</p>}
      {costEstimate !== undefined && (
        <p className="hint estimate-line" data-testid="estimate-line">
          {estimateText(costEstimate)}
        </p>
      )}
      <div className="button-row">
        <button
          type="button"
          className="primary-button"
          disabled={startReason !== null}
          aria-describedby={startReason ? "start-reason" : undefined}
          onClick={() => void launch("eagle-eye")}
        >
          Start Eagle-Eye
        </button>
        <button type="button" className="link-button" onClick={onPlainTab}>
          Plain terminal tab instead
        </button>
      </div>
      {startReason && (
        <p className="hint" id="start-reason" data-testid="start-reason">
          {startReason}
        </p>
      )}
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
                {run.adopted && <span className="hint"> (from the dev app)</span>}
                <span className="hint">
                  {" "}
                  — {stageText(run.stage)}
                  {run.folder ? ` · ${run.folder}` : ""}
                  {relativeDate(run.createdAt ?? run.updatedAt) ? ` · ${relativeDate(run.createdAt ?? run.updatedAt)}` : ""}
                </span>
                {run.stage === "done" && (
                  <>
                    {run.hasReport && (
                      <button type="button" className="link-button" onClick={() => void showReport(run)}>
                        Report
                      </button>
                    )}
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => void revealItemInDir(run.runDir).catch((err: unknown) => setError(errorText(err)))}
                    >
                      Open folder
                    </button>
                  </>
                )}
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
          {onShowAllRuns && (
            <button type="button" className="link-button" onClick={onShowAllRuns}>
              Show all{(overview?.runs.length ?? 0) > EARLIER_RUNS ? ` (${overview!.runs.length})` : ""}
            </button>
          )}
        </section>
      )}
      {duplicate && (
        <Dialog
          title="This run is already in progress"
          onClose={() => setDuplicate(null)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setDuplicate(null)}>
                Cancel
              </button>
              <button
                type="button"
                className="secondary-button"
                disabled={busy}
                onClick={() => {
                  const role = duplicate.role;
                  setDuplicate(null);
                  void launch(role, true);
                }}
              >
                Start anyway
              </button>
              <button
                type="button"
                className="primary-button"
                disabled={busy}
                onClick={() => {
                  const dir = duplicate.run.runDir;
                  setDuplicate(null);
                  void resume(dir);
                }}
              >
                Resume that run
              </button>
            </>
          }
        >
          <p>
            An unfinished run has the same request in the same folder{tickets ? " and the same tickets" : ""}.
            Resume it, or start a new one anyway.
          </p>
        </Dialog>
      )}
      {report && <TextDialog title={report.title} text={report.text} onClose={() => setReport(null)} />}
      {overview?.pluginVersion && <p className="hint run-version">dcv2 roles {overview.pluginVersion}</p>}
    </div>
  );
}
