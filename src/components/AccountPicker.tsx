import { useState } from "react";
import type { ResolvedClaudeAccount } from "../bridge";
import { Dialog } from "./Dialog";
import { errorText } from "./InlineNotice";

type Props = {
  accounts: ResolvedClaudeAccount[];
  /** This window's account, chosen first. */
  current: string;
  onOpen: (accountId: string) => Promise<void>;
  onClose: () => void;
};

/** New window: which Claude account it uses. */
export function AccountPicker({ accounts, current, onOpen, onClose }: Props) {
  const [choice, setChoice] = useState(
    accounts.some((account) => account.id === current) ? current : (accounts[0]?.id ?? ""),
  );
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    try {
      await onOpen(choice);
      onClose();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <Dialog
      title="New window"
      onClose={onClose}
      footer={
        <>
          <button type="button" className="secondary-button" onClick={onClose}>
            Cancel
          </button>
          <button type="button" className="primary-button" disabled={!choice} onClick={() => void submit()}>
            Open window
          </button>
        </>
      }
    >
      <p className="hint">Each window uses one Claude account. Two windows may use the same one.</p>
      <div role="radiogroup" aria-label="Claude account" className="account-choices">
        {accounts.map((account) => (
          <label key={account.id} className="account-choice">
            <input
              type="radio"
              name="account"
              checked={choice === account.id}
              onChange={() => setChoice(account.id)}
            />
            <span>{account.name}</span>
            <span className="hint">{account.config.display}</span>
          </label>
        ))}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}
