import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  claudeAccountLogins,
  claudeStatus,
  setClaudeSettings,
  notificationPermission,
  pushSendTest,
  pushSetSecret,
  pushStatus,
  setPushSettings,
  sendTestNotification,
  setNotifySettings,
  setObserveSettings,
  setRunSettings,
  setTerminalSettings,
  setUiSettings,
  type ClaudeAccount,
  type ClaudeAccountLogin,
  type ClaudeStatus,
  type LoginStatus,
  type NotifySettings,
  type ObserveSettings,
  type PushSettings,
  type RunSettings,
  type SettingsView,
  type TerminalSettings,
  type TriageScheduleRow,
} from "../bridge";
import { accountsProblem, addAccount, MAX_ACCOUNTS } from "../settings/accounts";
import { ATTENTION_KINDS, DEFAULT_EXPORT_ENDPOINT, PUSH_KINDS } from "../run/words";
import { currentSection, matchesFilter, SETTINGS_SECTIONS } from "../settings/sections";
import { THEME_CHOICES } from "../theme";
import { detectPlatform, revealLabel, type Platform } from "../terminal/keys";
import { Dialog } from "./Dialog";
import { useDialogFocus } from "./useDialogFocus";
import { errorText, InlineNotice } from "./InlineNotice";
import { ClickUpTokens } from "../clickup/ClickUpTokens";
import { WebQaTargets } from "./WebQaTargets";

type Props = {
  view: SettingsView;
  /** Saved: the parent applies it (window title, terminal options, theme). */
  onSaved: (view: SettingsView) => void;
  onClose: () => void;
  /** Scroll to this section (its heading, e.g. "Phone push") when the page opens. */
  section?: string;
  /** The OS, for the click modifier and the reveal option's name. Default: this one. */
  platform?: Platform;
};

/** A folder from the native picker, or null when cancelled. */
async function chooseFolder(start: string): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, defaultPath: start || undefined });
  return typeof picked === "string" ? picked : null;
}

/** One line for an account's `claude auth status`. */
export function loginText(login: LoginStatus): string {
  switch (login.state) {
    case "loggedIn": {
      const who = [login.account, login.method].filter(Boolean).join(" · ");
      return who ? `Signed in: ${who}` : "Signed in";
    }
    case "loggedOut":
      return "Not signed in. Run claude with this folder and use /login.";
    case "noCli":
      return "Claude Code (claude) was not found.";
    case "noConfigDir":
      return login.detail ?? "The config folder does not exist.";
    default:
      return login.detail ? `Unknown: ${login.detail}` : "Unknown";
  }
}

/** The model choices of the select (the backend decides what is valid; "" is the account's own model). */
const ORCHESTRATOR_MODELS = ["opus", "sonnet", "haiku", ""];

const DEFAULT_RUN: RunSettings = { maxImplementers: 3, stopHooks: true };

/** The comparable form of each section's values: defaults filled in, so an untouched field is never "changed". */
const accountsKey = (claudePath: string, accounts: ClaudeAccount[]) =>
  JSON.stringify({
    claudePath: claudePath.trim() || null,
    accounts: accounts.map((a) => ({
      id: a.id,
      name: a.name,
      configDir: a.configDir || null,
      capturePolicy: a.capturePolicy ?? "metadata",
      keepRunTraces: a.keepRunTraces ?? false,
    })),
  });
const terminalKey = (t: TerminalSettings) => JSON.stringify(t, Object.keys(t).sort());
const runKey = (r: RunSettings) =>
  JSON.stringify({
    maxImplementers: r.maxImplementers,
    stopHooks: r.stopHooks,
    slopCheck: r.slopCheck ?? true,
    autoCleanup: r.autoCleanup ?? true,
    watchdogMinutes: r.watchdogMinutes ?? 10,
    maxActiveRuns: r.maxActiveRuns ?? 2,
    usageHoldPercent: r.usageHoldPercent ?? null,
    batchPushes: r.batchPushes ?? true,
    autoStartWaves: r.autoStartWaves ?? false,
    triageSchedule: r.triageSchedule ?? [],
  });

/**
 * Settings › Runs › Triage schedule: per row a ClickUp list of an account, a local time and the start folder. At most one
 * triage run per row per day starts on its own, and none while the list's last proposal has not been answered.
 */
export function TriageSchedule({
  rows,
  accounts,
  onChange,
}: {
  rows: TriageScheduleRow[];
  accounts: { id: string; name: string }[];
  onChange: (rows: TriageScheduleRow[]) => void;
}) {
  const set = (at: number, patch: Partial<TriageScheduleRow>) => onChange(rows.map((r, i) => (i === at ? { ...r, ...patch } : r)));
  return (
    <fieldset className="triage-schedule" data-testid="triage-schedule">
      <legend>Triage schedule</legend>
      <p className="hint">
        Once a day at the time given, triage the open tickets of a ClickUp list (its proposal waits in Runs › Triage). None starts while the last
        proposal of that list has not been answered.
      </p>
      {rows.map((row, at) => (
        <div key={at} className="triage-schedule-row">
          <label className="check-label">
            <input type="checkbox" aria-label={`Triage schedule row ${at + 1} on`} checked={row.on} onChange={(event) => set(at, { on: event.target.checked })} /> On
          </label>
          <select className="text-input text-input-narrow" aria-label={`Triage schedule row ${at + 1} account`} value={row.accountId} onChange={(event) => set(at, { accountId: event.target.value })}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
          <input className="text-input text-input-narrow" aria-label={`Triage schedule row ${at + 1} list id`} placeholder="list id" value={row.listId} onChange={(event) => set(at, { listId: event.target.value })} />
          <input className="text-input text-input-narrow" type="time" aria-label={`Triage schedule row ${at + 1} time`} value={row.time} onChange={(event) => set(at, { time: event.target.value })} />
          <input className="text-input" aria-label={`Triage schedule row ${at + 1} folder`} placeholder="folder to start in" value={row.folder} onChange={(event) => set(at, { folder: event.target.value })} />
          <button type="button" className="link-button" aria-label={`Remove triage schedule row ${at + 1}`} onClick={() => onChange(rows.filter((_, i) => i !== at))}>
            Remove
          </button>
        </div>
      ))}
      <button
        type="button"
        className="secondary-button"
        onClick={() => onChange([...rows, { accountId: accounts[0]?.id ?? "", listId: "", time: "08:00", on: false, folder: "" }])}
      >
        Add a triage schedule row
      </button>
    </fieldset>
  );
}

/** "Unsaved changes" beside a section's Save button. */
function UnsavedMarker({ show }: { show: boolean }) {
  return show ? (
    <span className="unsaved-marker" role="status">
      Unsaved changes
    </span>
  ) : null;
}

/** "Accounts and Runs", "Accounts, Terminal and Runs". */
function sectionList(names: string[]): string {
  return names.length <= 1 ? names.join("") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/**
 * Settings, over the window: the Claude accounts (a name and a config folder
 * each), where `claude` is, the terminal, and the theme. Escape or Close goes
 * back to the tabs.
 */
export function SettingsPage({ view, onSaved, onClose, section, platform = detectPlatform(navigator.platform) }: Props) {
  const [accounts, setAccounts] = useState<ClaudeAccount[]>(view.claude.accounts);
  const [claudePath, setClaudePath] = useState(view.claude.claudePath ?? "");
  const [newName, setNewName] = useState("");
  const [newDir, setNewDir] = useState("");
  const [terminal, setTerminal] = useState<TerminalSettings>(view.terminal);
  const [runSettings, setRun] = useState<RunSettings>(view.run ?? DEFAULT_RUN);
  // The last saved values of the three Save sections, for the unsaved marker and the discard dialog.
  const [savedAccounts, setSavedAccounts] = useState(() => accountsKey(view.claude.claudePath ?? "", view.claude.accounts));
  const [savedTerminal, setSavedTerminal] = useState(() => terminalKey(view.terminal));
  const [savedRun, setSavedRun] = useState(() => runKey(view.run ?? DEFAULT_RUN));
  const [confirmDiscard, setConfirmDiscard] = useState(false);
  // The orchestrator rows save on change from the last saved values, so a pending edit elsewhere is not saved with them.
  const [savedRunValue, setSavedRunValue] = useState<RunSettings>(view.run ?? DEFAULT_RUN);
  const [savedAccountList, setSavedAccountList] = useState<ClaudeAccount[]>(view.claude.accounts);
  const [savedClaudePath, setSavedClaudePath] = useState(view.claude.claudePath ?? null);
  const [customModelOpen, setCustomModelOpen] = useState(false);
  const [customModel, setCustomModel] = useState("");
  const [modelError, setModelError] = useState<string | null>(null);
  const [accountModelError, setAccountModelError] = useState<{ id: string; text: string } | null>(null);
  const accountsDirty = accountsKey(claudePath, accounts) !== savedAccounts;
  const terminalDirty = terminalKey(terminal) !== savedTerminal;
  const runDirty = runKey(runSettings) !== savedRun;
  const unsaved = [
    accountsDirty && "Accounts",
    terminalDirty && "Terminal",
    runDirty && "Runs",
  ].filter((name): name is string => Boolean(name));
  // Escape and Close never lose edits silently.
  const requestClose = () => (unsaved.length > 0 ? setConfirmDiscard(true) : onClose());
  const [logins, setLogins] = useState<ClaudeAccountLogin[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [cli, setCli] = useState<ClaudeStatus | null>(null);
  const [notice, setNotice] = useState<{ tone: "info" | "error"; text: string } | null>(null);

  useEffect(() => {
    let current = true;
    claudeStatus()
      .then((status) => current && setCli(status))
      .catch((err: unknown) => current && setCli({ found: false, path: null, version: null, error: errorText(err) }));
    return () => {
      current = false;
    };
  }, []);

  const pageRef = useRef<HTMLDivElement>(null);
  const [filter, setFilter] = useState("");
  const [current, setCurrent] = useState(0);
  const sectionEls = () => [...(pageRef.current?.querySelectorAll<HTMLElement>("section.settings-section") ?? [])];
  const sectionByName = (name: string) => sectionEls().find((el) => el.getAttribute("aria-label") === name);
  // The filter only sets `hidden`: nothing unmounts, so an edit in a field the filter hides is kept.
  useEffect(() => {
    const query = filter.trim();
    for (const el of sectionEls()) {
      const children = [...el.children].filter((child): child is HTMLElement => child instanceof HTMLElement && child.tagName !== "H2");
      const titleMatches = query === "" || matchesFilter(el.getAttribute("aria-label") ?? "", query);
      let shown = 0;
      for (const child of children) {
        const names = [...child.querySelectorAll("[aria-label]")].map((node) => node.getAttribute("aria-label")).join(" ");
        const match = titleMatches || matchesFilter(`${child.textContent ?? ""} ${names}`, query);
        child.hidden = !match;
        if (match) shown += 1;
      }
      el.hidden = !titleMatches && shown === 0;
    }
  });
  const spy = () => {
    const page = pageRef.current;
    if (!page) return;
    const top = page.getBoundingClientRect().top;
    const tops = SETTINGS_SECTIONS.map((name) => {
      const el = sectionByName(name);
      return el && !el.hidden ? el.getBoundingClientRect().top - top + page.scrollTop : Number.POSITIVE_INFINITY;
    });
    setCurrent(currentSection(tops, page.scrollTop));
  };
  const goTo = (name: string, index: number) => {
    sectionByName(name)?.scrollIntoView?.({ block: "start" });
    setCurrent(index);
  };
  // Focus lands on the page itself so Escape works at once; it returns to the opener on close.
  useDialogFocus(pageRef, null);
  useEffect(() => {
    if (!section) return;
    const target = [...(pageRef.current?.querySelectorAll("section.settings-section") ?? [])].find(
      (el) => el.getAttribute("aria-label") === section,
    );
    target?.scrollIntoView?.({ block: "start" });
  }, [section]);

  const storedModel = runSettings.orchestratorModel ?? "opus";
  const resolved = (id: string) => view.accounts.find((account) => account.id === id);
  const edit = (index: number, patch: Partial<ClaudeAccount>) =>
    setAccounts((list) => list.map((account, at) => (at === index ? { ...account, ...patch } : account)));

  const saveAccounts = async () => {
    const problem = accountsProblem(accounts);
    if (problem) {
      setNotice({ tone: "error", text: problem });
      return;
    }
    try {
      const next = await setClaudeSettings({ claudePath: claudePath.trim() || null, accounts });
      setAccounts(next.claude.accounts);
      setSavedAccounts(accountsKey(claudePath, next.claude.accounts));
      setSavedAccountList(next.claude.accounts);
      setSavedClaudePath(next.claude.claudePath ?? null);
      setLogins(null);
      setNotice({ tone: "info", text: "Accounts saved." });
      onSaved(next);
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveRun = async () => {
    try {
      const saved = await setRunSettings(runSettings);
      setRun(saved);
      setSavedRunValue(saved);
      setSavedRun(runKey(saved));
      setNotice({ tone: "info", text: "Run settings saved." });
      onSaved({ ...view, run: saved });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  /** Saves one orchestrator setting of the Runs section on its own; a refusal shows the backend's message. */
  const saveOrchestrator = async (patch: Partial<RunSettings>): Promise<boolean> => {
    try {
      const saved = await setRunSettings({ ...savedRunValue, ...patch });
      setSavedRunValue(saved);
      setRun((run) => ({
        ...run,
        orchestratorModel: saved.orchestratorModel,
        orchestratorPermissionMode: saved.orchestratorPermissionMode,
      }));
      onSaved({ ...view, run: saved });
      setModelError(null);
      return true;
    } catch (err) {
      setModelError(errorText(err));
      return false;
    }
  };

  const saveAccountModel = async (id: string, model: string | null) => {
    try {
      const next = await setClaudeSettings({
        claudePath: savedClaudePath,
        accounts: savedAccountList.map((a) => (a.id === id ? { ...a, orchestratorModel: model } : a)),
      });
      setSavedAccountList(next.claude.accounts);
      setAccounts((list) =>
        list.map((a) => ({ ...a, orchestratorModel: next.claude.accounts.find((n) => n.id === a.id)?.orchestratorModel })),
      );
      setAccountModelError(null);
      onSaved(next);
    } catch (err) {
      setAccountModelError({ id, text: errorText(err) });
    }
  };

  const saveTerminal = async () => {
    try {
      const saved = await setTerminalSettings(terminal);
      setTerminal(saved);
      setSavedTerminal(terminalKey(saved));
      setNotice({ tone: "info", text: "Terminal settings saved." });
      onSaved({ ...view, terminal: saved });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const notify = view.notify ?? { master: true, events: {} };
  const [permission, setPermission] = useState<string | null>(null);
  const [testFailed, setTestFailed] = useState(false);

  useEffect(() => {
    let current = true;
    notificationPermission().then(
      (state) => current && setPermission(state),
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, []);

  const saveNotify = async (next: NotifySettings) => {
    try {
      const saved = await setNotifySettings(next);
      onSaved({ ...view, notify: saved });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const sendTest = async () => {
    setTestFailed(false);
    try {
      await sendTestNotification();
      setNotice({ tone: "info", text: "Test notification sent." });
    } catch (err) {
      setTestFailed(true);
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const push: PushSettings = view.push ?? { target: "none", ntfyServer: "https://ntfy.sh", events: {} };
  const [pushSaved, setPushSaved] = useState<string[]>([]);
  const [pushSecret, setPushSecret] = useState<Record<string, string>>({});
  const [pushResult, setPushResult] = useState<{ tone: "info" | "error"; text: string } | null>(null);

  useEffect(() => {
    let current = true;
    pushStatus().then(
      (status) => current && setPushSaved(status.saved),
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, []);

  const savePush = async (next: PushSettings) => {
    try {
      const saved = await setPushSettings(next);
      onSaved({ ...view, push: saved });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveSecret = async (name: string) => {
    try {
      const status = await pushSetSecret(name, pushSecret[name] ?? "");
      setPushSaved(status.saved);
      setPushSecret((current) => ({ ...current, [name]: "" }));
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const sendPushTest = async () => {
    setPushResult(null);
    try {
      await pushSendTest();
      setPushResult({ tone: "info", text: "Test push sent." });
    } catch (err) {
      setPushResult({ tone: "error", text: errorText(err) });
    }
  };

  const secretFields: Record<string, { name: string; label: string }[]> = {
    ntfy: [{ name: "ntfy-topic", label: "ntfy topic" }],
    pushover: [
      { name: "pushover-token", label: "Pushover app token" },
      { name: "pushover-user", label: "Pushover user key" },
    ],
    webhook: [{ name: "webhook-url", label: "Webhook URL" }],
  };

  const [editor, setEditor] = useState(view.ui.editorCommand ?? "");
  const [endpointDraft, setEndpointDraft] = useState(view.observe?.exportEndpoint ?? DEFAULT_EXPORT_ENDPOINT);

  const saveEditor = async () => {
    try {
      const ui = await setUiSettings({ ...view.ui, editorCommand: editor });
      setEditor(ui.editorCommand ?? "");
      onSaved({ ...view, ui });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveObserve = async (patch: Partial<ObserveSettings>, text: string) => {
    try {
      const observe = await setObserveSettings({
        hooksAllTabs: view.observe?.hooksAllTabs ?? true,
        evalEnabled: view.observe?.evalEnabled ?? false,
        ...(view.observe?.exportEndpoint ? { exportEndpoint: view.observe.exportEndpoint } : {}),
        ...patch,
      });
      setNotice({ tone: "info", text });
      onSaved({ ...view, observe });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveEndpoint = async () => {
    const wanted = endpointDraft.trim();
    if (wanted === (view.observe?.exportEndpoint ?? DEFAULT_EXPORT_ENDPOINT)) return;
    try {
      const observe = await setObserveSettings({
        hooksAllTabs: view.observe?.hooksAllTabs ?? true,
        evalEnabled: view.observe?.evalEnabled ?? false,
        exportEndpoint: wanted,
      });
      setEndpointDraft(observe.exportEndpoint ?? wanted);
      setNotice({ tone: "info", text: "Export endpoint saved." });
      onSaved({ ...view, observe });
    } catch (err) {
      // The old value is kept; say why this one was refused.
      setEndpointDraft(view.observe?.exportEndpoint ?? DEFAULT_EXPORT_ENDPOINT);
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveTheme = async (theme: string) => {
    try {
      const ui = await setUiSettings({ ...view.ui, theme });
      onSaved({ ...view, ui });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const checkLogins = async () => {
    setChecking(true);
    try {
      setLogins(await claudeAccountLogins());
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    } finally {
      setChecking(false);
    }
  };

  return (
    <div
      ref={pageRef}
      className="settings-page"
      role="dialog"
      aria-label="Settings"
      tabIndex={-1}
      onScroll={spy}
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        requestClose();
      }}
    >
      <header className="settings-header">
        <h1>Settings</h1>
        <button type="button" className="secondary-button" onClick={requestClose}>
          Close
        </button>
      </header>
      {confirmDiscard && (
        <Dialog
          title="Discard unsaved changes?"
          onClose={() => setConfirmDiscard(false)}
          footer={
            <>
              <button type="button" className="primary-button" onClick={() => setConfirmDiscard(false)}>
                Keep editing
              </button>
              <button type="button" className="secondary-button" onClick={onClose}>
                Discard
              </button>
            </>
          }
        >
          <p>
            {sectionList(unsaved)} {unsaved.length === 1 ? "has" : "have"} unsaved changes.
          </p>
        </Dialog>
      )}
      {notice && <InlineNotice tone={notice.tone}>{notice.text}</InlineNotice>}
      <nav className="settings-index" aria-label="Settings sections">
        <input
          type="search"
          className="text-input"
          aria-label="Filter settings"
          placeholder="Filter settings"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        <ul>
          {SETTINGS_SECTIONS.map((name, index) => (
            <li key={name}>
              <button
                type="button"
                className="settings-index-item"
                aria-current={current === index ? "true" : undefined}
                onClick={() => goTo(name, index)}
              >
                {name}
              </button>
            </li>
          ))}
        </ul>
      </nav>

      <section className="settings-section" aria-label="Claude accounts">
        <h2>Claude accounts</h2>
        <p className="hint">
          Each window uses one account: a name and its Claude config folder (CLAUDE_CONFIG_DIR). An
          empty folder is ~/.claude. DCTerminalv2 never creates or writes these folders.
        </p>
        {view.configDirEnv && (
          <InlineNotice tone="warning">
            DCTV2_CLAUDE_CONFIG_DIR is set: the first account uses {view.configDirEnv}.
          </InlineNotice>
        )}
        <ul className="account-list">
          {accounts.map((account, index) => {
            const info = resolved(account.id);
            const login = logins?.find((item) => item.id === account.id);
            return (
              <li key={account.id} className="account-row">
                <input
                  className="text-input"
                  aria-label="Account name"
                  value={account.name}
                  onChange={(event) => edit(index, { name: event.target.value })}
                />
                <span className="field-row">
                  <input
                    className="text-input"
                    aria-label={`Config folder of ${account.name}`}
                    placeholder="~/.claude"
                    value={account.configDir ?? ""}
                    onChange={(event) => edit(index, { configDir: event.target.value || null })}
                  />
                  <button
                    type="button"
                    className="secondary-button"
                    onClick={() =>
                      void chooseFolder(account.configDir ?? "").then((dir) => dir && edit(index, { configDir: dir }))
                    }
                  >
                    Choose…
                  </button>
                  <button
                    type="button"
                    className="danger-button"
                    disabled={accounts.length <= 1}
                    title={accounts.length <= 1 ? "Keep at least one account" : undefined}
                    onClick={() => setAccounts((list) => list.filter((_, at) => at !== index))}
                  >
                    Remove
                  </button>
                </span>
                {info && (
                  <span className={info.config.exists ? "hint" : "hint warn"}>
                    {info.config.path}
                    {info.config.exists ? "" : " (not found)"}
                    {info.envOverride ? " (from DCTV2_CLAUDE_CONFIG_DIR)" : ""}
                  </span>
                )}
                {login && <span className="hint">{loginText(login.login)}</span>}
              </li>
            );
          })}
        </ul>
        <div className="account-add">
          <input
            className="text-input"
            aria-label="New account name"
            placeholder="Name, e.g. Work"
            value={newName}
            onChange={(event) => setNewName(event.target.value)}
          />
          <span className="field-row">
            <input
              className="text-input"
              aria-label="New account config folder"
              placeholder="~/.claude-account2"
              value={newDir}
              onChange={(event) => setNewDir(event.target.value)}
            />
            <button
              type="button"
              className="secondary-button"
              onClick={() => void chooseFolder(newDir).then((dir) => dir && setNewDir(dir))}
            >
              Choose…
            </button>
            <button
              type="button"
              className="secondary-button"
              disabled={!newName.trim() || accounts.length >= MAX_ACCOUNTS}
              onClick={() => {
                setAccounts((list) => addAccount(list, newName, newDir));
                setNewName("");
                setNewDir("");
              }}
            >
              Add
            </button>
          </span>
        </div>
        <label className="field-label">
          Claude Code path (empty: found on PATH)
          <input
            className="text-input"
            value={claudePath}
            placeholder="claude"
            onChange={(event) => setClaudePath(event.target.value)}
          />
        </label>
        {cli && (
          <p className={cli.found ? "hint" : "error"} data-testid="claude-status">
            {cli.found ? `Claude Code: ${cli.version ?? "version unknown"} (${cli.path})` : cli.error}
          </p>
        )}
        <div className="button-row">
          <button type="button" className="primary-button" onClick={() => void saveAccounts()}>
            Save accounts
          </button>
          <UnsavedMarker show={accountsDirty} />
          <button type="button" className="secondary-button" disabled={checking} onClick={() => void checkLogins()}>
            {checking ? "Checking…" : "Check logins"}
          </button>
        </div>
      </section>

      <ClickUpTokens accounts={view.claude.accounts} autoPost={view.clickup?.autoPost ?? {}} />

      <section className="settings-section" aria-label="Terminal">
        <h2>Terminal</h2>
        <label className="field-label">
          Shell (empty: your login shell)
          <input
            className="text-input"
            value={terminal.shell}
            onChange={(event) => setTerminal({ ...terminal, shell: event.target.value })}
          />
        </label>
        <label className="field-label">
          Font size
          <input
            className="text-input text-input-narrow"
            type="number"
            min={8}
            max={32}
            value={terminal.fontSize}
            onChange={(event) => setTerminal({ ...terminal, fontSize: Number(event.target.value) || 14 })}
          />
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={terminal.optionAsMeta}
            onChange={(event) => setTerminal({ ...terminal, optionAsMeta: event.target.checked })}
          />{" "}
          Option key acts as Meta (macOS)
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={terminal.copyOnSelect}
            onChange={(event) => setTerminal({ ...terminal, copyOnSelect: event.target.checked })}
          />{" "}
          Copy a selection to the clipboard
        </label>
        <label className="field-label">
          {platform === "mac" ? "Cmd" : "Ctrl"}+click on a file path in the terminal opens it in
          <select
            className="text-input text-input-narrow"
            value={terminal.fileLinkEditor ?? "finder"}
            onChange={(event) => setTerminal({ ...terminal, fileLinkEditor: event.target.value as NonNullable<TerminalSettings["fileLinkEditor"]> })}
          >
            <option value="finder">{revealLabel(platform)}</option>
            <option value="vscode">VS Code</option>
            <option value="cursor">Cursor</option>
            <option value="zed">Zed</option>
            <option value="sublime">Sublime Text</option>
          </select>
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={terminal.compactPaste ?? true}
            onChange={(event) => setTerminal({ ...terminal, compactPaste: event.target.checked })}
          />{" "}
          Offer to compact a big paste (above 200 lines or 16 KB; you choose each time)
        </label>
        <div className="button-row">
          <button type="button" className="primary-button" onClick={() => void saveTerminal()}>
            Save terminal settings
          </button>
          <UnsavedMarker show={terminalDirty} />
        </div>
      </section>

      <section className="settings-section" aria-label="Runs">
        <h2>Runs</h2>
        <label className="field-label">
          Implementers at once (Eagle-Eye)
          <input
            className="text-input text-input-narrow"
            type="number"
            min={1}
            max={8}
            value={runSettings.maxImplementers}
            onChange={(event) => setRun({ ...runSettings, maxImplementers: Number(event.target.value) || 3 })}
          />
        </label>
        <label className="field-label">
          Orchestrator model
          <select
            className="text-input"
            value={customModelOpen || !ORCHESTRATOR_MODELS.includes(storedModel) ? "custom" : storedModel}
            aria-describedby="orchestrator-model-hint"
            onChange={(event) => {
              const value = event.target.value;
              if (value === "custom") {
                setCustomModelOpen(true);
                return;
              }
              setCustomModelOpen(false);
              setModelError(null);
              void saveOrchestrator({ orchestratorModel: value });
            }}
          >
            <option value="opus">Opus (default)</option>
            <option value="sonnet">Sonnet</option>
            <option value="haiku">Haiku</option>
            <option value="">Account default</option>
            <option value="custom">Custom model id</option>
          </select>
        </label>
        {(customModelOpen || !ORCHESTRATOR_MODELS.includes(storedModel)) && (
          <label className="field-label">
            Model id
            <input
              className="text-input"
              value={customModelOpen ? customModel : storedModel}
              placeholder="claude-…"
              aria-invalid={modelError ? true : undefined}
              aria-describedby={modelError ? "orchestrator-model-error" : undefined}
              onChange={(event) => {
                setCustomModelOpen(true);
                setCustomModel(event.target.value);
              }}
              onBlur={() => {
                if (!customModelOpen || customModel.trim() === "") return;
                void saveOrchestrator({ orchestratorModel: customModel.trim() }).then((ok) => ok && setCustomModelOpen(false));
              }}
            />
          </label>
        )}
        {modelError && (
          <p className="hint warn" id="orchestrator-model-error" role="alert">
            {modelError}
          </p>
        )}
        <p className="hint" id="orchestrator-model-hint">
          The model every Eagle-Eye run starts on. Role and plain tabs are not changed.
        </p>
        <label className="field-label">
          Orchestrator permission mode
          <select
            className="text-input"
            value={runSettings.orchestratorPermissionMode ?? "auto"}
            onChange={(event) => void saveOrchestrator({ orchestratorPermissionMode: event.target.value })}
          >
            <option value="auto">Auto (default)</option>
            <option value="acceptEdits">Accept edits</option>
            <option value="default">Ask every time</option>
            <option value="">Account default</option>
          </select>
        </label>
        {accounts.map((account) => (
          <div key={account.id}>
            <label className="field-label">
              Orchestrator model of {account.name}
              <select
                className="text-input"
                aria-label={`Orchestrator model of ${account.name}`}
                value={account.orchestratorModel ?? ""}
                onChange={(event) => void saveAccountModel(account.id, event.target.value || null)}
              >
                <option value="">Use the run setting</option>
                <option value="opus">Opus</option>
                <option value="sonnet">Sonnet</option>
                <option value="haiku">Haiku</option>
                {account.orchestratorModel && !ORCHESTRATOR_MODELS.includes(account.orchestratorModel) && (
                  <option value={account.orchestratorModel}>{account.orchestratorModel}</option>
                )}
              </select>
            </label>
            {accountModelError?.id === account.id && (
              <p className="hint warn" role="alert">
                {accountModelError.text}
              </p>
            )}
          </div>
        ))}
        <label className="field-label">
          Runs live at once (the rest wait in the queue)
          <input
            className="text-input text-input-narrow"
            type="number"
            min={1}
            max={8}
            value={runSettings.maxActiveRuns ?? 2}
            onChange={(event) => setRun({ ...runSettings, maxActiveRuns: Number(event.target.value) || 2 })}
          />
        </label>
        <label className="field-label">
          Hold queued starts at this five-hour usage percent (empty is off)
          <input
            className="text-input text-input-narrow"
            type="number"
            min={1}
            max={100}
            value={runSettings.usageHoldPercent ?? ""}
            onChange={(event) => setRun({ ...runSettings, usageHoldPercent: event.target.value === "" ? null : Number(event.target.value) })}
          />
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={runSettings.batchPushes ?? true}
            onChange={(event) => setRun({ ...runSettings, batchPushes: event.target.checked })}
          />{" "}
          Phone pushes when a batch has every plan ready, or is ready to ship (the Mac is always told)
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={runSettings.autoStartWaves ?? false}
            onChange={(event) => setRun({ ...runSettings, autoStartWaves: event.target.checked })}
          />{" "}
          Start a ready wave on its own (through the run limit)
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={runSettings.stopHooks}
            onChange={(event) => setRun({ ...runSettings, stopHooks: event.target.checked })}
          />{" "}
          Run the repo's checks when an agent stops (stop hooks)
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={runSettings.slopCheck ?? true}
            onChange={(event) => setRun({ ...runSettings, slopCheck: event.target.checked })}
          />{" "}
          Also report slop in the lines an agent added (leftover debugging, filler comments) when it stops
        </label>
        <label className="check-label">
          <input
            type="checkbox"
            checked={runSettings.autoCleanup ?? true}
            onChange={(event) => setRun({ ...runSettings, autoCleanup: event.target.checked })}
          />{" "}
          Clean up merged runs automatically (worktree, local and remote branch of each merged PR)
        </label>
        <label className="field-label">
          Say “no progress” after this many quiet minutes (0 is off)
          <input
            className="text-input text-input-narrow"
            type="number"
            min={0}
            max={240}
            value={runSettings.watchdogMinutes ?? 10}
            onChange={(event) => {
              const minutes = Number(event.target.value);
              setRun({ ...runSettings, watchdogMinutes: Number.isFinite(minutes) && minutes >= 0 ? minutes : 10 });
            }}
          />
        </label>
        <TriageSchedule
          rows={runSettings.triageSchedule ?? []}
          accounts={view.accounts.map((a) => ({ id: a.id, name: a.name }))}
          onChange={(triageSchedule) => setRun({ ...runSettings, triageSchedule })}
        />
        <div className="button-row">
          <button type="button" className="primary-button" onClick={() => void saveRun()}>
            Save run settings
          </button>
          <UnsavedMarker show={runDirty} />
        </div>
        <label className="field-label">
          Editor for “Open in editor” on a PR (program and arguments; empty: code)
          <input
            className="text-input"
            aria-label="Editor command"
            placeholder="code"
            value={editor}
            onChange={(event) => setEditor(event.target.value)}
            onBlur={() => void saveEditor()}
          />
        </label>
      </section>

      <WebQaTargets targets={view.webQa?.targets ?? []} onSaved={(webQa) => onSaved({ ...view, webQa })} />

      <section className="settings-section" aria-label="Activity">
        <h2>Activity</h2>
        <label className="check-label">
          <input
            type="checkbox"
            checked={view.observe?.hooksAllTabs ?? true}
            onChange={(event) =>
              void saveObserve(
                { hooksAllTabs: event.target.checked },
                "Saved. New Claude tabs use it; open ones keep what they started with.",
              )
            }
          />{" "}
          Track activity in every Claude tab
        </label>
        <p className="hint">
          Adds a small hook to every Claude tab so its dot shows working, waiting or needs you.
        </p>
        <label className="check-label">
          <input
            type="checkbox"
            checked={view.observe?.evalEnabled ?? false}
            onChange={(event) =>
              void saveObserve(
                { evalEnabled: event.target.checked },
                event.target.checked ? "Eval mode is on." : "Eval mode is off.",
              )
            }
          />{" "}
          Eval mode
        </label>
        <p className="hint">Runs the same request several times on your own subscription. Nothing is pushed or posted.</p>
        <label className="field-label">
          Export endpoint
          <input
            className="text-input"
            aria-label="Export endpoint"
            value={endpointDraft}
            onChange={(event) => setEndpointDraft(event.target.value)}
            onBlur={() => void saveEndpoint()}
            onKeyDown={(event) => {
              if (event.key === "Enter") void saveEndpoint();
            }}
          />
        </label>
        <p className="hint">
          Where Export sends a trace by default (OTLP over http or https). Phoenix listens on{" "}
          {DEFAULT_EXPORT_ENDPOINT} by default.
        </p>
        <h3>Session Inspector</h3>
        <p className="hint">
          What the trace may read from each account&apos;s session logs. Content is shown only when you open an event
          and is always secret-scanned; Off reads nothing. Saved with the accounts.
        </p>
        {accounts.map((account, index) => (
          <div key={account.id}>
            <label className="field-label">
              Capture policy of {account.name}
              <select
                className="text-input"
                aria-label={`Capture policy of ${account.name}`}
                value={account.capturePolicy ?? "metadata"}
                onChange={(event) => edit(index, { capturePolicy: event.target.value as ClaudeAccount["capturePolicy"] })}
              >
                <option value="off">Off (read nothing)</option>
                <option value="metadata">Metadata (default)</option>
                <option value="redacted">Redacted</option>
                <option value="full">Full</option>
              </select>
            </label>
            <label className="check-label">
              <input
                type="checkbox"
                aria-label={`Keep run traces of ${account.name}`}
                checked={account.capturePolicy !== "off" && (account.keepRunTraces ?? false)}
                disabled={account.capturePolicy === "off"}
                onChange={(event) => edit(index, { keepRunTraces: event.target.checked })}
              />{" "}
              Keep run traces
            </label>
            {account.capturePolicy === "off" && <p className="hint">AgentFlight is off for this account.</p>}
          </div>
        ))}
        <p className="hint">
          Keep run traces saves a finished run&apos;s session events with the run, so its Trace still opens after
          Claude deletes its logs. It applies to runs that finish after you turn it on.
        </p>
        <div className="button-row">
          <button
            type="button"
            className="primary-button"
            disabled={!accountsDirty}
            onClick={() => void saveAccounts()}
          >
            Save capture policy
          </button>
          <UnsavedMarker show={accountsDirty} />
        </div>
      </section>

      <section className="settings-section" aria-label="Notifications">
        <h2>Notifications</h2>
        <p className="hint">
          Tells you when a tab needs you, a run waits, stalls or finishes, or an app check fails. The dock shows how
          many tabs need you.
        </p>
        {(permission === "denied" || testFailed) && (
          <InlineNotice tone="warning">
            macOS blocks notifications for DCTerminalv2: allow them in System Settings &gt; Notifications.
          </InlineNotice>
        )}
        <label className="check-label">
          <input
            type="checkbox"
            checked={notify.master}
            onChange={(event) => void saveNotify({ ...notify, master: event.target.checked })}
          />{" "}
          Notify me (master switch: notices, system notifications, dock badge and bounce)
        </label>
        {ATTENTION_KINDS.map((kind) => (
          <label key={kind.key} className="check-label">
            <input
              type="checkbox"
              disabled={!notify.master}
              checked={notify.events[kind.key] ?? true}
              onChange={(event) =>
                void saveNotify({ ...notify, events: { ...notify.events, [kind.key]: event.target.checked } })
              }
            />{" "}
            {kind.label}
          </label>
        ))}
        <div className="button-row">
          <button type="button" className="secondary-button" onClick={() => void sendTest()}>
            Send a test notification
          </button>
        </div>
      </section>

      <section className="settings-section" aria-label="Phone push">
        <h2>Phone push</h2>
        <p className="hint">
          Sends the run id and the event kind (nothing else) to your phone when you are away from the Mac. The topic,
          tokens and URL are kept in the system keychain, not in settings.
        </p>
        <label className="field-label">
          Send to
          <select
            className="text-input text-input-narrow"
            value={push.target}
            onChange={(event) => void savePush({ ...push, target: event.target.value })}
          >
            <option value="none">Off</option>
            <option value="ntfy">ntfy</option>
            <option value="pushover">Pushover</option>
            <option value="webhook">Webhook</option>
          </select>
        </label>
        {push.target === "ntfy" && (
          <label className="field-label">
            ntfy server
            <input
              className="text-input"
              defaultValue={push.ntfyServer}
              onBlur={(event) => void savePush({ ...push, ntfyServer: event.target.value })}
            />
          </label>
        )}
        {(secretFields[push.target] ?? []).map((field) => (
          <div key={field.name} className="field-row">
            <label className="field-label">
              {field.label}
              <input
                className="text-input"
                type="password"
                autoComplete="off"
                placeholder={pushSaved.includes(field.name) ? "Saved in the keychain" : ""}
                value={pushSecret[field.name] ?? ""}
                onChange={(event) => setPushSecret({ ...pushSecret, [field.name]: event.target.value })}
              />
            </label>
            <button
              type="button"
              className="secondary-button"
              aria-label={`Save ${field.label}`}
              onClick={() => void saveSecret(field.name)}
            >
              {(pushSecret[field.name] ?? "") === "" && pushSaved.includes(field.name) ? "Clear" : "Save"}
            </button>
          </div>
        ))}
        {push.target !== "none" && (
          <>
            {PUSH_KINDS.map((kind) => (
              <label key={kind.key} className="check-label">
                <input
                  type="checkbox"
                  checked={push.events[kind.key] ?? true}
                  onChange={(event) =>
                    void savePush({ ...push, events: { ...push.events, [kind.key]: event.target.checked } })
                  }
                />{" "}
                {kind.label}
              </label>
            ))}
            <div className="button-row">
              <button type="button" className="secondary-button" onClick={() => void sendPushTest()}>
                Send test
              </button>
            </div>
            {pushResult && <InlineNotice tone={pushResult.tone}>{pushResult.text}</InlineNotice>}
          </>
        )}
      </section>

      <section className="settings-section" aria-label="Appearance">
        <h2>Appearance</h2>
        <label className="field-label">
          Theme
          <select
            className="text-input text-input-narrow"
            value={view.ui.theme}
            onChange={(event) => void saveTheme(event.target.value)}
          >
            {THEME_CHOICES.map((choice) => (
              <option key={choice.id} value={choice.id}>
                {choice.label}
              </option>
            ))}
          </select>
        </label>
      </section>
    </div>
  );
}
