import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  claudeAccountLogins,
  claudeStatus,
  setClaudeSettings,
  setRunSettings,
  setTerminalSettings,
  setUiSettings,
  type ClaudeAccount,
  type ClaudeAccountLogin,
  type ClaudeStatus,
  type LoginStatus,
  type RunSettings,
  type SettingsView,
  type TerminalSettings,
} from "../bridge";
import { accountsProblem, addAccount, MAX_ACCOUNTS } from "../settings/accounts";
import { THEME_CHOICES } from "../theme";
import { errorText, InlineNotice } from "./InlineNotice";

type Props = {
  view: SettingsView;
  /** Saved: the parent applies it (window title, terminal options, theme). */
  onSaved: (view: SettingsView) => void;
  onClose: () => void;
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

/**
 * Settings, over the window: the Claude accounts (a name and a config folder
 * each), where `claude` is, the terminal, and the theme. Escape or Close goes
 * back to the tabs.
 */
export function SettingsPage({ view, onSaved, onClose }: Props) {
  const [accounts, setAccounts] = useState<ClaudeAccount[]>(view.claude.accounts);
  const [claudePath, setClaudePath] = useState(view.claude.claudePath ?? "");
  const [newName, setNewName] = useState("");
  const [newDir, setNewDir] = useState("");
  const [terminal, setTerminal] = useState<TerminalSettings>(view.terminal);
  const [runSettings, setRun] = useState<RunSettings>(view.run ?? { maxImplementers: 3, stopHooks: true });
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
      setNotice({ tone: "info", text: "Run settings saved." });
      onSaved({ ...view, run: saved });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveTerminal = async () => {
    try {
      const saved = await setTerminalSettings(terminal);
      setTerminal(saved);
      setNotice({ tone: "info", text: "Terminal settings saved." });
      onSaved({ ...view, terminal: saved });
    } catch (err) {
      setNotice({ tone: "error", text: errorText(err) });
    }
  };

  const saveTheme = async (theme: string) => {
    try {
      const ui = await setUiSettings({ theme });
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
      className="settings-page"
      role="dialog"
      aria-label="Settings"
      onKeyDown={(event) => {
        if (event.key !== "Escape") return;
        event.preventDefault();
        onClose();
      }}
    >
      <header className="settings-header">
        <h1>Settings</h1>
        <button type="button" className="secondary-button" onClick={onClose}>
          Close
        </button>
      </header>
      {notice && <InlineNotice tone={notice.tone}>{notice.text}</InlineNotice>}

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
          <button type="button" className="secondary-button" disabled={checking} onClick={() => void checkLogins()}>
            {checking ? "Checking…" : "Check logins"}
          </button>
        </div>
      </section>

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
        <div className="button-row">
          <button type="button" className="primary-button" onClick={() => void saveTerminal()}>
            Save terminal settings
          </button>
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
        <label className="check-label">
          <input
            type="checkbox"
            checked={runSettings.stopHooks}
            onChange={(event) => setRun({ ...runSettings, stopHooks: event.target.checked })}
          />{" "}
          Run the repo's checks when an agent stops (stop hooks)
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
        <div className="button-row">
          <button type="button" className="primary-button" onClick={() => void saveRun()}>
            Save run settings
          </button>
        </div>
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
