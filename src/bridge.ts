/**
 * Typed wrappers for the Tauri commands the app registers (src-tauri lib.rs).
 * Every argument is camelCase here; Tauri maps it to the Rust snake_case
 * parameter. A window's commands name the window (`windowId`), so two
 * windows never share a focus.
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import type { WindowHistory } from "./usage/limits";

export type TabKind = "claude" | "shell";

export type TabSummary = {
  id: string;
  label: string;
  customLabel: boolean;
  cwd: string;
  kind: TabKind;
  /** `ok`, `missing`, `not-a-directory`, `unreadable` or `empty`. */
  folderStatus: string;
  /** A Claude tab that started a session before (it may be resumable). */
  hasSession: boolean;
  /** Eagle-Eye tabs: the run folder the progress panel reads. */
  runDir?: string | null;
  /** dcv2 tabs: `eagle-eye`, `eagle-eye-resume`, or the role. */
  role?: string | null;
  /** The second pane (a Shell) when the tab is split. */
  split?: SplitSummary | null;
};

export type SplitDirection = "right" | "down";

export type SplitSummary = {
  direction: SplitDirection;
  /** The first pane's share of the tab, 0.2 to 0.8. */
  ratio: number;
  /** The second pane's terminal id (`<tab id>:p2`). */
  id: string;
  cwd: string;
  folderStatus: string;
};

export type ClosedTabSummary = {
  id: string;
  label: string;
  cwd: string;
  kind: TabKind;
};

export type AppStateSnapshot = {
  windowId: string;
  activeTabId: string | null;
  tabs: TabSummary[];
  /** This window's closed tabs, newest first. */
  closedTabs: ClosedTabSummary[];
};

export type ConfigDirInfo = {
  path: string;
  /** Short form with the home folder as `~`. */
  display: string;
  source: "env" | "setting" | "default";
  exists: boolean;
};

export type WindowContext = {
  id: string;
  accountId: string;
  accountName: string;
  title: string;
  config: ConfigDirInfo;
  /** The window's account was removed from Settings: its Claude tabs do not start. */
  accountMissing: boolean;
};

export type TerminalSettings = {
  shell: string;
  fontSize: number;
  optionAsMeta: boolean;
  copyOnSelect: boolean;
};

export type UiSettings = {
  theme: string;
  /** "Open in editor" on a PR card: program and arguments, no shell. Empty: `code`. */
  editorCommand?: string;
};

export type ClaudeAccount = {
  id: string;
  name: string;
  configDir?: string | null;
};

export type ClaudeSettings = {
  claudePath?: string | null;
  accounts: ClaudeAccount[];
};

export type ResolvedClaudeAccount = {
  id: string;
  name: string;
  config: ConfigDirInfo;
  /** The first account while `DCTV2_CLAUDE_CONFIG_DIR` is set. */
  envOverride: boolean;
};

export type RunSettings = {
  maxImplementers: number;
  stopHooks: boolean;
  watchdogMinutes?: number;
  /** "Clean up merged runs automatically" (default on). */
  autoCleanup?: boolean;
};

export type ObserveSettings = {
  /** "Track activity in every Claude tab" (default on). */
  hooksAllTabs: boolean;
};

/** Notifications: a master switch and one switch per event kind (missing: on). */
export type NotifySettings = {
  master: boolean;
  events: Record<string, boolean>;
};

/** Phone push: where it goes. The secrets live in the keychain and are never here. */
export type PushSettings = {
  /** `none`, `ntfy`, `pushover` or `webhook`. */
  target: string;
  ntfyServer: string;
  events: Record<string, boolean>;
};

export type PushStatus = { saved: string[] };

export type SettingsView = {
  terminal: TerminalSettings;
  ui: UiSettings;
  claude: ClaudeSettings;
  /** Eagle-Eye launch settings; absent from older test fixtures. */
  run?: RunSettings;
  observe?: ObserveSettings;
  notify?: NotifySettings;
  push?: PushSettings;
  /** Per account: auto-post ClickUp comments after ship approval and the user's click (missing: off). */
  clickup?: { autoPost: Record<string, boolean> };
  accounts: ResolvedClaudeAccount[];
  configDirEnv: string | null;
};

export type LoginStatus = {
  state: "loggedIn" | "loggedOut" | "unknown" | "noCli" | "noConfigDir" | string;
  account?: string | null;
  detail?: string | null;
  apiKeyEnv: boolean;
  method?: string | null;
  organization?: string | null;
};

export type ClaudeAccountLogin = {
  id: string;
  name: string;
  config: ConfigDirInfo;
  login: LoginStatus;
};

export type ClaudeStatus = {
  found: boolean;
  path: string | null;
  version: string | null;
  error: string | null;
};

export type RateWindow = {
  rateLimitType: string;
  label: string;
  utilization: number | null;
  resetsAt: number | null;
  status: string;
  seenAtMs: number;
};

export type UsageSnapshot = {
  configDir: string;
  windows: RateWindow[];
  /** The saved readings per window type, oldest first (at most 48 each). Absent in older fixtures. */
  history?: WindowHistory[];
};

export type PtyPacket = {
  kind: string;
  data: string;
  code: number | null;
};

export function windowContext(): Promise<WindowContext> {
  return invoke<WindowContext>("window_context");
}

/** Move this window to `accountId` (after its own account was removed). */
export function setWindowAccount(accountId: string): Promise<WindowContext> {
  return invoke<WindowContext>("set_window_account", { accountId });
}

/** Another window on `accountId`. Returns its label. */
export function openAccountWindow(accountId: string): Promise<string> {
  return invoke<string>("open_account_window", { accountId });
}

export function reopenClosedWindow(): Promise<string | null> {
  return invoke<string | null>("reopen_closed_window");
}

export function getAppState(windowId: string): Promise<AppStateSnapshot> {
  return invoke<AppStateSnapshot>("get_app_state", { windowId });
}

/** A new tab in `cwd`, made active. Its terminal starts when a view shows it. */
export function newTab(windowId: string, cwd: string, kind: TabKind): Promise<TabSummary> {
  return invoke<TabSummary>("new_tab", { windowId, cwd, kind });
}

export function selectTab(windowId: string, tabId: string): Promise<void> {
  return invoke<void>("select_tab", { windowId, tabId });
}

/** Drags `tabId` to `index` among the window's tabs; resolves to the window's new state. */
export function moveTab(windowId: string, tabId: string, index: number): Promise<AppStateSnapshot> {
  return invoke<AppStateSnapshot>("move_tab", { windowId, tabId, index });
}

/** Up to 10 existing folders for the New tab and Start forms, newest first. */
export function recentFolders(): Promise<string[]> {
  return invoke<string[]>("recent_folders");
}

/** Points a tab whose folder is gone at another folder; it keeps its id (and its name unless it follows the folder). */
export function setTabFolder(tabId: string, cwd: string): Promise<TabSummary> {
  return invoke<TabSummary>("set_tab_folder", { tabId, cwd });
}

/** Splits the tab: a second Shell pane, right of it or below it. */
export function splitTab(windowId: string, tabId: string, direction: SplitDirection): Promise<TabSummary> {
  return invoke<TabSummary>("split_tab", { windowId, tabId, direction });
}

/** Closes pane 1 or 2 of a split tab and stops its terminal. */
export function closePane(windowId: string, tabId: string, pane: 1 | 2): Promise<TabSummary> {
  return invoke<TabSummary>("close_pane", { windowId, tabId, pane });
}

/** The divider moved: the first pane's share of the tab. */
export function setSplitRatio(tabId: string, ratio: number): Promise<TabSummary> {
  return invoke<TabSummary>("set_split_ratio", { tabId, ratio });
}

export function renameTab(tabId: string, label: string): Promise<TabSummary> {
  return invoke<TabSummary>("rename_tab", { tabId, label });
}

/** Stops the tab's terminal and moves the tab to the closed list. */
export function closeTab(windowId: string, tabId: string): Promise<AppStateSnapshot> {
  return invoke<AppStateSnapshot>("close_tab", { windowId, tabId });
}

/** `tabId`, or the window's most recently closed tab when null. */
export function reopenClosedTab(windowId: string, tabId: string | null = null): Promise<TabSummary> {
  return invoke<TabSummary>("reopen_closed_tab", { windowId, tabId });
}

export function createPtyChannel(onPacket: (packet: PtyPacket) => void): Channel<PtyPacket> {
  const channel = new Channel<PtyPacket>();
  channel.onmessage = onPacket;
  return channel;
}

/**
 * Start the terminal of tab `id` (what runs comes from the tab record).
 * `resume`: a Claude tab resumes its saved session instead of starting fresh.
 */
export function ptyOpen(input: {
  id: string;
  cols: number;
  rows: number;
  resume: boolean;
  onOutput: Channel<PtyPacket>;
}): Promise<number> {
  const { onOutput, ...rest } = input;
  return invoke<number>("pty_open", { input: rest, onOutput });
}

export function ptyWrite(id: string, data: string): Promise<void> {
  return invoke<void>("pty_write", { id, data });
}

export function ptyResize(id: string, cols: number, rows: number): Promise<void> {
  return invoke<void>("pty_resize", { id, cols, rows });
}

export function ptyKill(id: string): Promise<void> {
  return invoke<void>("pty_kill", { id });
}

/** Tells Claude Code (COLORFGBG) whether the app theme is light. */
export function ptySetScheme(light: boolean): Promise<void> {
  return invoke<void>("pty_set_scheme", { light });
}

/** The session the Claude tab can resume now, if any. */
export function terminalResumeId(tabId: string): Promise<string | null> {
  return invoke<string | null>("terminal_resume_id", { tabId });
}

export function getSettings(): Promise<SettingsView> {
  return invoke<SettingsView>("get_settings");
}

export function setTerminalSettings(terminal: TerminalSettings): Promise<TerminalSettings> {
  return invoke<TerminalSettings>("set_terminal_settings", { terminal });
}

export function setUiSettings(ui: UiSettings): Promise<UiSettings> {
  return invoke<UiSettings>("set_ui_settings", { ui });
}

export function setRunSettings(run: RunSettings): Promise<RunSettings> {
  return invoke<RunSettings>("set_run_settings", { run });
}

export function setObserveSettings(observe: ObserveSettings): Promise<ObserveSettings> {
  return invoke<ObserveSettings>("set_observe_settings", { observe });
}

export function setNotifySettings(notify: NotifySettings): Promise<NotifySettings> {
  return invoke<NotifySettings>("set_notify_settings", { notify });
}

/** One system notification and one in-app notice. Rejects when the system refuses. */
export function sendTestNotification(): Promise<void> {
  return invoke<void>("send_test_notification");
}

/** `granted`, `denied` or `prompt`. */
export function notificationPermission(): Promise<string> {
  return invoke<string>("notification_permission");
}

/** Selects the tab in its window and brings that window forward (null: the tab is gone). */
export function focusTab(tabId: string): Promise<TabSummary | null> {
  return invoke<TabSummary | null>("focus_tab_command", { tabId });
}

export function setPushSettings(push: PushSettings): Promise<PushSettings> {
  return invoke<PushSettings>("set_push_settings", { push });
}

/** Which push secrets are saved (names only). */
export function pushStatus(): Promise<PushStatus> {
  return invoke<PushStatus>("push_status");
}

/** Saves one push secret in the keychain; an empty value clears it. */
export function pushSetSecret(name: string, value: string): Promise<PushStatus> {
  return invoke<PushStatus>("push_set_secret", { name, value });
}

/** Sends one push; rejects with the reason when it did not arrive. */
export function pushSendTest(): Promise<void> {
  return invoke<void>("push_send_test");
}

/** One unfinished run tab, from the attention detector (any window). */
export type ActiveRun = {
  windowLabel: string;
  tabId: string;
  label: string;
  activity: string;
  runId: string | null;
  runDir: string | null;
  stage: string | null;
  waitingFor: string | null;
  stalled: boolean;
  prsDone: number;
  prsTotal: number;
  /** Per-epic progress (plugin 0.8.0 runs with 2+ epics); absent or empty: a flat count. */
  epics?: { id: string | null; title: string | null; done: number; total: number }[];
};

export type AttentionSummary = { needsYou: number; activeRuns: number };

export function activeRuns(): Promise<ActiveRun[]> {
  return invoke<ActiveRun[]>("active_runs");
}

export function attentionSummary(): Promise<AttentionSummary> {
  return invoke<AttentionSummary>("attention_summary");
}

export function setClaudeSettings(claude: ClaudeSettings): Promise<SettingsView> {
  return invoke<SettingsView>("set_claude_settings", { claude });
}

/** Signed-in state per account. Slow: each check runs `claude auth status`. */
export function claudeAccountLogins(): Promise<ClaudeAccountLogin[]> {
  return invoke<ClaudeAccountLogin[]>("claude_account_logins");
}

export function claudeStatus(): Promise<ClaudeStatus> {
  return invoke<ClaudeStatus>("claude_status");
}

export function getClaudeUsage(windowId: string): Promise<UsageSnapshot> {
  return invoke<UsageSnapshot>("get_claude_usage", { windowId });
}
