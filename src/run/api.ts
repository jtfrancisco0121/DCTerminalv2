/**
 * Typed wrappers for the run commands (src-tauri/src/run/mod.rs): the start
 * screen, the dcv2 launch, the progress panel, R1 checks, the worktree
 * janitor and the memory panel.
 */

import { invoke } from "@tauri-apps/api/core";
import type { TabSummary } from "../bridge";
import type { ClickUpStart } from "../clickup/api";

export type Repo = {
  name: string;
  path: string;
  check: string | null;
  /** Worktree setup command (app setting); null: the plugin detects it from the lockfile. */
  setup?: string | null;
  /** Git-ignored files copied from the main checkout into run worktrees. */
  copyFiles?: string[];
  /** Live Claude tests allowed in this repo's checks (repos.json `liveTests`). */
  liveTests?: boolean;
};

/** One repo's worktree setup (app settings, keyed by the repo's real path). */
export type RepoSetup = { setup: string | null; copyFiles: string[]; liveTests?: boolean };

/** `github`: draft PRs + one review comment per round; `local`: the quiet local loop. */
export type ReviewMode = "github" | "local";
/** BMAD planning (BMAD-METHOD skills): when the Planner writes a spec and an architecture before the plan. */
export type BmadMode = "auto" | "on" | "off";

export type FolderRepos = {
  /** `repo`, `workspace`, `inside-repo` or `none`. */
  kind: string;
  repos: Repo[];
};

export type RoleTile = { id: string; label: string };

/** What the Runs view shows from a finished run's record (`app-usage.json`). */
export type RecordTotals = {
  outcome: string | null;
  totalTokens: number;
  /** An estimate at list prices; null when a model has no price. */
  estimateUsd: number | null;
  /** Active minutes over all stages. */
  minutes: number;
  prUrls: string[];
  firstActivity: string | null;
  lastActivity: string | null;
};

export type RunListItem = {
  runId: string;
  runDir: string;
  folder: string | null;
  createdAt: string | null;
  stage: string | null;
  updatedAt: string | null;
  title: string | null;
  /** A dev-app run used in place (Import from dev app). */
  adopted?: boolean;
  /** Absent for a run without a record (old, or not finished). */
  record?: RecordTotals | null;
  prCount?: number | null;
  /** ClickUp ticket ids and custom ids of the run. */
  ticketIds?: string[];
  hasReport?: boolean;
  /** Repos the run works on (request.md `repos:` or repos.json). */
  repoCount?: number | null;
  /** BMAD planning was applied (null: not decided yet). */
  bmadApplied?: boolean | null;
};

export type RunOverview = {
  pluginDir: string | null;
  pluginVersion: string | null;
  roles: RoleTile[];
  runs: RunListItem[];
};

export type GateFile = {
  gate: string | null;
  reason: string | null;
  updated: string | null;
  problem: string | null;
};

export type AppCheck = {
  /** `pass`, `fail`, `timeout`, `error`, `none`, `running`, `interrupted`. */
  result: string;
  /** The command the app ran (repos.json's). */
  command: string | null;
  /** status.json's command when it differs (never run by the app). */
  skillCommand?: string | null;
  /** The worktree's HEAD and clean flag when the check ran. */
  head?: string | null;
  clean?: boolean | null;
  cwd: string | null;
  exitCode: number | null;
  finishedAt: string | null;
  log: string | null;
  detail: string | null;
};

export type GateStatus = {
  /** `proven`, `claimed`, `review-only`, `concerns`, `fail`, `unreadable`, `none`. */
  kind: string;
  text: string;
};

export type PrView = {
  id: string;
  title: string | null;
  repo: string | null;
  repoPath: string | null;
  branch: string | null;
  worktree: string | null;
  state: string | null;
  note: string | null;
  storiesDone: number | null;
  storiesTotal: number | null;
  check: { command: string | null; result: string | null; summary: string | null; cwd: string | null };
  review: { round: number | null; verdict: string | null; openFindings: number | null };
  prUrl: string | null;
  gate: GateFile | null;
  appCheck: AppCheck | null;
  gateStatus: GateStatus | null;
  /** repos.json's check for the PR's repo: the only command the app runs. */
  appCommand: string | null;
  checkKnown: boolean;
  currentHead: string | null;
  worktreeClean: boolean | null;
  /** The app's check ran on another HEAD than the worktree's now. */
  appCheckStale: boolean;
  /** Review on GitHub: true while the PR is a draft. Missing on older runs. */
  draft?: boolean | null;
  /** One PR comment per review round. */
  comments?: { round: number | null; url: string }[];
  /** GitHub CI (app-github.json): `pass`, `fail`, `pending`, `none`, `unknown`; null until read. */
  ci?: string | null;
  /** When the app read the PR on GitHub (UTC ISO). */
  ciAt?: string | null;
  /** Why the last `gh` call failed. */
  ciError?: string | null;
  /** The PR's state on GitHub: `OPEN`, `MERGED`, `CLOSED`; null until read. */
  prState?: string | null;
  /** The PR's epic (status.json `prs[].epic`, plugin 0.8.0+); absent on older runs. */
  epic?: { id: string; title: string | null } | null;
  /** The PR this one is stacked on (`prs[].stackedOn`); absent when not stacked. */
  stackedOn?: string | null;
  /** The branch this PR opens against (`prs[].base`). */
  base?: string | null;
  /** The PRs this one depends on (`prs[].dependsOn`). */
  dependsOn?: string[];
};

/** BMAD planning of a run (auto or on); absent for older runs and runs with it off. */
export type BmadView = {
  mode: string;
  /** The vendored BMAD pin the run started with. */
  version: string | null;
  size: string | null;
  reasons: string[];
  applied: boolean;
  /** `pass` or `fail`, from the last bmad-check. */
  checkResult: string | null;
  /** Run-relative paths of the documents (open with readRunFile). */
  documents: string[];
};

export type LogLine = { time: string | null; stage: string | null; text: string };

export type RunView = {
  runId: string;
  runDir: string;
  folder: string | null;
  hasStatus: boolean;
  statusProblem: string | null;
  stage: string | null;
  waitingFor: string | null;
  pausedReason: string | null;
  error: string | null;
  updatedAt: string | null;
  plan: {
    approved: boolean;
    verdict: string | null;
    reviewRound: number | null;
    openFindings: number | null;
    /** The last Risk Reviewer round (`plan.risk`); null for a run from before it. */
    riskVerdict: string | null;
    riskOpenFindings: number | null;
  } | null;
  prs: PrView[];
  shipNote: string | null;
  timeline: LogLine[];
  hasReport: boolean;
  /** meta.json's review mode; missing is `local`. */
  reviewMode?: string;
  /** status.json has `shipApproval`. */
  shipApproved?: boolean;
  /** BMAD planning; missing or null when the run has it off (or is older). */
  bmad?: BmadView | null;
  /** What the app cleaned up for merged PRs (app-cleanup.json). */
  cleanup?: CleanupRecord[];
};

/** One PR's merged-run cleanup (outcome `cleaned`, `kept` or `untouched`). */
export type CleanupRecord = {
  prId: string;
  prUrl: string;
  outcome: string;
  note?: string;
  /** Not final: the app asks again later (a dirty worktree, a network failure). */
  retry?: boolean;
  worktree: string;
  branch: string;
  remote: string;
  at: string;
};

export type WorktreeItem = {
  prId: string;
  worktree: string;
  repoPath: string | null;
  branch: string | null;
  exists: boolean;
  clean: boolean;
  changedFiles: number;
  removable: boolean;
  /** Removable; its ignored build/dependency folders go with it. */
  disposableOnly?: boolean;
  problem: string | null;
};

export type RemoveResult = { prId: string; removed: boolean; branchDeleted: boolean; message: string };

export type MemoryTopic = { name: string; checkedAt?: string | null; stale?: boolean };
export type MemoryRepo = {
  name?: string;
  path?: string;
  id?: string;
  present?: boolean;
  staleCount?: number;
  topics?: MemoryTopic[];
};
export type MemoryStatus = {
  available: boolean;
  problem?: string;
  workspace?: { present?: boolean; path?: string } | null;
  repos?: MemoryRepo[];
};

export type StartedRun = {
  tab: TabSummary;
  runDir: string | null;
  /** A tab already running this run was focused (no second launch). */
  focused?: boolean;
};

export const runOverview = () => invoke<RunOverview>("run_overview");
export const detectFolderRepos = (folder: string) => invoke<FolderRepos>("detect_folder_repos", { folder });
/** `role`: `eagle-eye` or a role tile id. */
export const startRun = (
  windowId: string,
  folder: string,
  request: string,
  role: string,
  clickup: ClickUpStart | null = null,
  reviewMode: ReviewMode | null = null,
  bmadMode: BmadMode | null = null,
  force = false,
  /** The repo names ticked on the start screen (null: every repo). */
  repos: string[] | null = null,
  /** This run's implementer count (null: the setting). */
  maxImplementers: number | null = null,
) =>
  invoke<StartedRun>("start_run", {
    windowId,
    folder,
    request,
    role,
    clickup,
    reviewMode,
    bmadMode,
    force,
    repos,
    maxImplementers,
  });

/** An unfinished run with the same request, folder and tickets (what `start_run` answers instead of starting). */
export type DuplicateRun = { runId: string; runDir: string };

/** The duplicate a failed `startRun` names, else null. */
export function duplicateOf(err: unknown): DuplicateRun | null {
  if (!err || typeof err !== "object") return null;
  const dup = (err as { duplicate?: Partial<DuplicateRun> }).duplicate;
  return dup && typeof dup.runId === "string" && typeof dup.runDir === "string"
    ? { runId: dup.runId, runDir: dup.runDir }
    : null;
}
export const setRepoSetup = (repoPath: string, setup: RepoSetup) =>
  invoke<RepoSetup>("set_repo_setup", { repoPath, setup });
export const resumeRun = (windowId: string, runDir: string) => invoke<StartedRun>("resume_run", { windowId, runDir });
export const readRun = (runDir: string) => invoke<RunView>("read_run", { runDir });
export const rerunChecks = (runDir: string) => invoke<void>("rerun_checks", { runDir });
export const runWorktrees = (runDir: string) => invoke<WorktreeItem[]>("run_worktrees", { runDir });
/** Whether each PR's worktree passes the backend's check (where the contract puts it, on the run's branch). */
export type WorktreeCheck = { prId: string; verified: boolean; reason: string | null };
export const prWorktreeChecks = (runDir: string) => invoke<WorktreeCheck[]>("pr_worktree_checks", { runDir });
export type PrWorktreeAction = "reveal" | "editor" | "shell";
/** `shell` opens a Shell tab in `windowId` and resolves to it. */
export const prWorktreeAction = (runDir: string, prId: string, action: PrWorktreeAction, windowId?: string) =>
  invoke<TabSummary | null>("pr_worktree_action", { runDir, prId, action, windowId });
export const removeRunWorktrees = (runDir: string, prIds: string[]) =>
  invoke<RemoveResult[]>("remove_run_worktrees", { runDir, prIds });
export const readRunFile = (runDir: string, path: string) => invoke<string>("read_run_file", { runDir, path });
export const memoryStatus = (folder: string) => invoke<MemoryStatus>("memory_status", { folder });
export const readMemoryFile = (path: string) => invoke<string>("read_memory_file", { path });
