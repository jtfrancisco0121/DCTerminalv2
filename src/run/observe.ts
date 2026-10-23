/**
 * Typed wrappers for the P3b observe commands (src-tauri/src/observe/):
 * hook state per tab, the session log (sub-agents, cost, context), the
 * watchdog, Nudge, the Claude Code version range and its self-test.
 */

import { invoke } from "@tauri-apps/api/core";

/** `unknown`, `working`, `waiting`, `needs-you`. */
export type TabActivity = "unknown" | "working" | "waiting" | "needs-you";

export type TabHookState = {
  activity: TabActivity;
  lastEvent: string | null;
  updatedMs: number | null;
  subagentsRunning: number;
};

export type Tokens = { input: number; output: number; cacheWrite5m: number; cacheWrite1h: number; cacheRead: number };

export type CostLine = { name: string; tokens: Tokens; totalTokens: number; costUsd: number | null; minutes: number };

export type UsageSummary = {
  stages: CostLine[];
  agents: CostLine[];
  totalTokens: number;
  totalCostUsd: number | null;
  minutes: number;
  messages: number;
};

export type ContextMeter = { model: string; used: number; window: number; percent: number; warn: boolean };

export type SubagentInfo = {
  toolUseId: string;
  agentId: string | null;
  agentType: string;
  description: string | null;
  status: string;
  background: boolean;
  startedMs: number | null;
  finishedMs: number | null;
};

export type Stall = { minutes: number; threshold: number };

export type TabObservation = {
  hook: TabHookState | null;
  hooksActive: boolean;
  transcriptFound: boolean;
  context: ContextMeter | null;
  subagents: SubagentInfo[];
  usage: UsageSummary | null;
  stall: Stall | null;
};

export type SelfTest = {
  version: string | null;
  ok: boolean;
  pluginLoaded: boolean;
  agentSpawned: boolean;
  agentCompleted: boolean;
  finished: boolean;
  at: string;
  detail: string | null;
  seconds: number;
};

export type ClaudeVersionStatus = {
  version: string | null;
  /** `ok`, `below`, `above` or `unknown`. */
  range: string;
  min: string;
  maxTested: string;
  selftest: SelfTest | null;
  selftestWanted: boolean;
};

export const observeTab = (tabId: string) => invoke<TabObservation>("observe_tab", { tabId });
export const observeTabs = () => invoke<Record<string, TabHookState>>("observe_tabs");
/** Types `continue from status.json` + Enter into the tab. Only on a click. */
export const nudgeTab = (tabId: string) => invoke<void>("nudge_tab", { tabId });
export const claudeVersionStatus = (windowId: string) => invoke<ClaudeVersionStatus>("claude_version_status", { windowId });
export const runClaudeSelftest = (windowId: string) => invoke<SelfTest>("run_claude_selftest", { windowId });
export const readCheckLog = (runDir: string, prId: string) => invoke<string>("read_check_log", { runDir, prId });

export function activityText(activity: TabActivity | undefined | null): string | null {
  switch (activity) {
    case "working":
      return "working";
    case "waiting":
      return "waiting for you";
    case "needs-you":
      return "needs you";
    default:
      return null;
  }
}

export function tokensText(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return String(n);
}

export function costText(usd: number | null): string {
  if (usd === null) return "$ ?";
  if (usd < 0.01) return "<$0.01";
  return `$${usd.toFixed(2)}`;
}

/** `logs/<PR>-app-check.txt` → `<PR>`, else null. */
export function checkLogPr(path: string): string | null {
  const match = /^logs\/(.+)-app-check\.txt$/.exec(path);
  return match ? match[1] : null;
}

/** The version banner's text, or null when no banner is needed. */
export function versionWarning(status: ClaudeVersionStatus): string | null {
  if (status.range === "below")
    return `Claude Code ${status.version ?? ""} is older than ${status.min}, the oldest this app supports. Update Claude Code.`;
  if (status.range === "above")
    return `Claude Code ${status.version ?? ""} is newer than ${status.maxTested}, the newest this app was tested with. Runs may misbehave; the self-test checks the basics.`;
  return null;
}
