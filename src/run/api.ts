/**
 * Typed wrappers for the run commands (src-tauri/src/run/mod.rs): the start
 * screen, the dcv2 launch, the progress panel, R1 checks, the worktree
 * janitor and the memory panel.
 */

import { invoke } from "@tauri-apps/api/core";
import type { TabSummary } from "../bridge";

export type Repo = { name: string; path: string; check: string | null };

export type FolderRepos = {
  /** `repo`, `workspace`, `inside-repo` or `none`. */
  kind: string;
  repos: Repo[];
};

export type RoleTile = { id: string; label: string };

export type RunListItem = {
  runId: string;
  runDir: string;
  folder: string | null;
  createdAt: string | null;
  stage: string | null;
  updatedAt: string | null;
  title: string | null;
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
  plan: { approved: boolean; verdict: string | null; reviewRound: number | null; openFindings: number | null } | null;
  prs: PrView[];
  shipNote: string | null;
  timeline: LogLine[];
  hasReport: boolean;
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
export const startRun = (windowId: string, folder: string, request: string, role: string) =>
  invoke<StartedRun>("start_run", { windowId, folder, request, role });
export const resumeRun = (windowId: string, runDir: string) => invoke<StartedRun>("resume_run", { windowId, runDir });
export const readRun = (runDir: string) => invoke<RunView>("read_run", { runDir });
export const rerunChecks = (runDir: string) => invoke<void>("rerun_checks", { runDir });
export const runWorktrees = (runDir: string) => invoke<WorktreeItem[]>("run_worktrees", { runDir });
export const removeRunWorktrees = (runDir: string, prIds: string[]) =>
  invoke<RemoveResult[]>("remove_run_worktrees", { runDir, prIds });
export const readRunFile = (runDir: string, path: string) => invoke<string>("read_run_file", { runDir, path });
export const memoryStatus = (folder: string) => invoke<MemoryStatus>("memory_status", { folder });
export const readMemoryFile = (path: string) => invoke<string>("read_memory_file", { path });
