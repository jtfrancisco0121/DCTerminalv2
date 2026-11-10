import { useEffect, useState } from "react";
import type { WebQaCharter, WebQaSettings, WebQaTarget } from "../bridge";
import { webqaPasswordClear, webqaPasswordSet, webqaPasswordStatus, webqaTargetsSave, type PasswordStatus } from "../run/api";
import { Dialog } from "./Dialog";
import { errorText, InlineNotice } from "./InlineNotice";

type Props = {
  targets: WebQaTarget[];
  /** The saved list, as the app tidied it. */
  onSaved: (settings: WebQaSettings) => void;
};

const byTitle = (list: WebQaCharter[]) => [...list].sort((a, b) => a.title.toLowerCase().localeCompare(b.title.toLowerCase()));

/** A URL the test may open: http or https, nothing else. The app checks the rest. */
export function allowlistProblem(lines: string[]): string | null {
  for (const line of lines) {
    if (!/^https?:\/\/\S+$/i.test(line)) return `${line} is not a URL the test can use. Use http:// or https://.`;
  }
  return lines.length === 0 ? "Add at least one URL the test may open." : null;
}

const blank = (): WebQaTarget => ({ id: "", name: "", allowlist: [], username: "", clickupListId: null, charters: [] });

/**
 * Settings › Web QA: the targets the operator can test from outside (a deployed web app), each with its allowlist, a test
 * account (user name here, password in the macOS keychain) and the charters to try.
 */
export function WebQaTargets({ targets, onSaved }: Props) {
  const [list, setList] = useState<WebQaTarget[]>(targets);
  const [editing, setEditing] = useState<WebQaTarget | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [removing, setRemoving] = useState<WebQaTarget | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const persist = async (next: WebQaTarget[]) => {
    setNotice(null);
    try {
      const saved = await webqaTargetsSave(next);
      setList(saved.targets);
      onSaved(saved);
      return saved.targets;
    } catch (err) {
      setNotice(errorText(err));
      return null;
    }
  };

  return (
    <section className="settings-section" aria-label="Web QA">
      <h2>Web QA</h2>
      <p className="hint">
        Test a deployed web app from outside, like a person would. Each target lists the only URLs the test may open, a test
        account and the charters to try. Findings become ClickUp tasks only when you tick them in Runs.
      </p>
      {notice && !editing && <InlineNotice tone="error">{notice}</InlineNotice>}
      {list.length === 0 && !editing && <p className="hint">No targets yet. Add one to test a web app from outside.</p>}
      <ul className="account-list">
        {list.map((target) =>
          editing && !isNew && editing.id === target.id ? null : (
            <li key={target.id} className="account-row webqa-target">
              <strong>{target.name}</strong>
              <span className="hint">
                {target.allowlist.join(", ")} · {target.charters.length} charter{target.charters.length === 1 ? "" : "s"}
                {target.username ? ` · test account ${target.username}` : ""}
              </span>
              <span className="field-row">
                <button type="button" className="secondary-button" onClick={() => (setEditing(structuredClone(target)), setIsNew(false), setNotice(null))}>
                  Edit
                </button>
                <button type="button" className="danger-button" onClick={() => setRemoving(target)}>
                  Remove
                </button>
              </span>
            </li>
          ),
        )}
      </ul>
      {editing ? (
        <TargetForm
          key={editing.id || "new"}
          initial={editing}
          isNew={isNew}
          notice={notice}
          onCancel={() => (setEditing(null), setNotice(null))}
          onSave={async (target) => {
            const next = isNew ? [...list, target] : list.map((t) => (t.id === target.id ? target : t));
            const saved = await persist(next);
            if (saved) setEditing(null);
          }}
        />
      ) : (
        <button type="button" className="secondary-button" onClick={() => (setEditing(blank()), setIsNew(true), setNotice(null))}>
          Add target
        </button>
      )}
      {removing && (
        <Dialog
          title="Remove web QA target"
          onClose={() => setRemoving(null)}
          footer={
            <>
              <button type="button" className="secondary-button" onClick={() => setRemoving(null)}>
                Cancel
              </button>
              <button
                type="button"
                className="danger-button"
                onClick={() => {
                  const gone = removing;
                  setRemoving(null);
                  void persist(list.filter((t) => t.id !== gone.id));
                }}
              >
                Remove target
              </button>
            </>
          }
        >
          <p>Remove target {removing.name}? Its saved charters go too.</p>
        </Dialog>
      )}
    </section>
  );
}

function TargetForm({
  initial,
  isNew,
  notice,
  onCancel,
  onSave,
}: {
  initial: WebQaTarget;
  isNew: boolean;
  notice: string | null;
  onCancel: () => void;
  onSave: (target: WebQaTarget) => Promise<void>;
}) {
  const [name, setName] = useState(initial.name);
  const [urls, setUrls] = useState(initial.allowlist.join("\n"));
  const [username, setUsername] = useState(initial.username);
  const [listId, setListId] = useState(initial.clickupListId ?? "");
  const [charters, setCharters] = useState<WebQaCharter[]>(byTitle(initial.charters));
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [password, setPassword] = useState("");
  const [pw, setPw] = useState<PasswordStatus | null>(null);
  const [pwNote, setPwNote] = useState<string | null>(null);

  useEffect(() => {
    if (isNew || !initial.id) return;
    webqaPasswordStatus(initial.id).then(setPw, (err: unknown) => setPw({ targetId: initial.id, saved: false, problem: errorText(err) }));
  }, [initial.id, isNew]);

  const editCharter = (index: number, patch: Partial<WebQaCharter>) =>
    setCharters((rows) => rows.map((row, at) => (at === index ? { ...row, ...patch } : row)));

  const save = async () => {
    const lines = urls.split("\n").map((l) => l.trim()).filter(Boolean);
    const bad = allowlistProblem(lines);
    if (bad) return setProblem(bad);
    if (!name.trim()) return setProblem("Give the target a name.");
    setProblem(null);
    setBusy(true);
    await onSave({ ...initial, name: name.trim(), allowlist: lines, username: username.trim(), clickupListId: listId.trim() || null, charters });
    setBusy(false);
  };

  const keychainDown = pw?.problem ?? null;
  const setPassword_ = async () => {
    setPwNote(null);
    try {
      const status = await webqaPasswordSet(initial.id, password);
      setPw(status);
      setPassword("");
      setPwNote("Password saved in the keychain.");
    } catch (err) {
      setPwNote(errorText(err));
    }
  };

  return (
    <div className="account-add webqa-form" role="group" aria-label={isNew ? "New web QA target" : `Edit ${initial.name}`}>
      {(problem || notice) && <InlineNotice tone="error">{problem ?? notice}</InlineNotice>}
      <label className="field-label">
        Name
        <input className="text-input" aria-label="Target name" value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="field-label">
        URLs the test may open (one per line)
        <textarea className="text-input" aria-label="Allowed URLs" rows={3} value={urls} onChange={(e) => setUrls(e.target.value)} />
      </label>
      <p className="hint">
        Only these URLs can be opened. Anything else is refused. Add the hosts the app loads scripts, styles or login pages from
        too.
      </p>
      <label className="field-label">
        Test account user name
        <input className="text-input" aria-label="Test account user name" value={username} onChange={(e) => setUsername(e.target.value)} />
      </label>
      <label className="field-label">
        Test account password
        <span className="field-row">
          <input
            className="text-input"
            type="password"
            autoComplete="off"
            aria-label="Test account password"
            value={password}
            disabled={isNew || !!keychainDown}
            onChange={(e) => setPassword(e.target.value)}
          />
          <button type="button" className="secondary-button" disabled={isNew || !!keychainDown || !password} onClick={() => void setPassword_()}>
            Save password
          </button>
          <button
            type="button"
            className="secondary-button"
            disabled={isNew || !!keychainDown || !pw?.saved}
            onClick={() => webqaPasswordClear(initial.id).then((s) => (setPw(s), setPwNote("Password removed.")), (e: unknown) => setPwNote(errorText(e)))}
          >
            Remove password
          </button>
        </span>
      </label>
      <p className="hint">
        Kept in the macOS keychain. Use a test account, never your own.
        {isNew && " Save the target first, then add the password."}
        {keychainDown && ` The keychain is not available: ${keychainDown}`}
        {pw && !keychainDown && !isNew && (pw.saved ? " A password is saved." : " No password is saved.")}
      </p>
      {pwNote && <p className="hint">{pwNote}</p>}
      <label className="field-label">
        ClickUp list for findings (optional)
        <input className="text-input" aria-label="ClickUp list id" value={listId} onChange={(e) => setListId(e.target.value)} />
      </label>
      <h3>Charters</h3>
      <ul className="account-list">
        {charters.map((charter, index) => (
          <li key={charter.id || `new-${index}`} className="account-row">
            <input
              className="text-input"
              aria-label="Charter title"
              placeholder="Sign in"
              value={charter.title}
              onChange={(e) => editCharter(index, { title: e.target.value })}
            />
            <textarea
              className="text-input"
              aria-label="Charter text"
              rows={2}
              placeholder="What a person should be able to do"
              value={charter.text}
              onChange={(e) => editCharter(index, { text: e.target.value })}
            />
            <label className="check-label">
              <input
                type="checkbox"
                checked={charter.mayChangeData}
                onChange={(e) => editCharter(index, { mayChangeData: e.target.checked })}
              />
              This charter may change data
            </label>
            <button type="button" className="danger-button" onClick={() => setCharters((rows) => rows.filter((_, at) => at !== index))}>
              Remove charter
            </button>
          </li>
        ))}
      </ul>
      <button
        type="button"
        className="secondary-button"
        onClick={() => setCharters((rows) => [...rows, { id: "", title: "", text: "", mayChangeData: false }])}
      >
        Add charter
      </button>
      <div className="field-row">
        <button type="button" className="primary-button" disabled={busy} onClick={() => void save()}>
          Save target
        </button>
        <button type="button" className="secondary-button" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}
