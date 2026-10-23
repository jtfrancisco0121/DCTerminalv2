import { useEffect, useState } from "react";
import { errorText } from "../components/InlineNotice";
import { clickupClearToken, clickupSetToken, clickupTestToken, clickupTokenStatus, type TokenStatus } from "./api";

type Account = { id: string; name: string };

/**
 * Settings › ClickUp: one token per Claude account, kept in the macOS
 * keychain. The token is never shown again, logged, written to a file or
 * given to Claude.
 */
export function ClickUpTokens({ accounts }: { accounts: Account[] }) {
  return (
    <section className="settings-section" aria-label="ClickUp">
      <h2>ClickUp</h2>
      <p className="hint">
        Each account uses its own ClickUp personal token (ClickUp › Settings › Apps), kept in the macOS keychain. Only
        the app talks to ClickUp; Claude never gets the token.
      </p>
      <ul className="account-list">
        {accounts.map((account) => (
          <TokenRow key={account.id} account={account} />
        ))}
      </ul>
    </section>
  );
}

function TokenRow({ account }: { account: Account }) {
  const [status, setStatus] = useState<TokenStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [note, setNote] = useState<{ tone: "info" | "error"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () =>
    clickupTokenStatus(account.id).then(setStatus, (err: unknown) =>
      setNote({ tone: "error", text: errorText(err) }),
    );

  useEffect(() => {
    void refresh();
  }, [account.id]);

  const run = async (work: () => Promise<string>) => {
    setBusy(true);
    setNote(null);
    try {
      setNote({ tone: "info", text: await work() });
    } catch (err) {
      setNote({ tone: "error", text: errorText(err) });
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  const save = () =>
    run(async () => {
      await clickupSetToken(account.id, draft);
      setDraft("");
      return "Token saved in the keychain.";
    });
  const test = () =>
    run(async () => {
      const result = await clickupTestToken(account.id);
      return result.workspaces.length
        ? `The token works. Workspaces: ${result.workspaces.join(", ")}.`
        : "The token works, but it sees no workspace.";
    });
  const clear = () =>
    run(async () => {
      await clickupClearToken(account.id);
      return "Token removed from the keychain.";
    });

  return (
    <li className="account-row" data-testid={`clickup-${account.id}`}>
      <strong>{account.name}</strong>{" "}
      <span className="hint">
        {status === null ? "…" : status.problem ? status.problem : status.saved ? "token saved" : "no token"}
      </span>
      <span className="field-row">
        <input
          className="text-input"
          type="password"
          autoComplete="off"
          aria-label={`ClickUp token for ${account.name}`}
          placeholder={status?.saved ? "Paste a new token to replace it" : "pk_…"}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
        />
        <button type="button" className="secondary-button" disabled={busy || !draft.trim()} onClick={() => void save()}>
          Save
        </button>
        <button type="button" className="secondary-button" disabled={busy || !status?.saved} onClick={() => void test()}>
          Test
        </button>
        <button type="button" className="secondary-button" disabled={busy || !status?.saved} onClick={() => void clear()}>
          Clear
        </button>
      </span>
      {note && <p className={note.tone === "error" ? "error" : "hint"}>{note.text}</p>}
    </li>
  );
}
