/**
 * Editing the Claude accounts list (Settings). An account is a name and a
 * Claude config folder; its id is made once from the name and never shown.
 * The backend checks the list again on save (`normalize_claude_accounts`).
 */

import type { ClaudeAccount } from "../bridge";

/** Most accounts Settings keeps (the backend's limit). */
export const MAX_ACCOUNTS = 8;

/**
 * A safe, unused id for a new account named `name`: lower-case letters,
 * digits and dashes from the name (`Work 2` → `work-2`), else `account`,
 * with `-2`, `-3`, … added while it is taken.
 */
export function accountIdFor(name: string, taken: readonly string[]): string {
  const base =
    name
      .normalize("NFKD")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 40) || "account";
  if (!taken.includes(base)) return base;
  for (let n = 2; ; n += 1) {
    const id = `${base}-${n}`;
    if (!taken.includes(id)) return id;
  }
}

/** The list with a new account added at the end. */
export function addAccount(accounts: readonly ClaudeAccount[], name: string, configDir: string): ClaudeAccount[] {
  const trimmed = name.trim() || "Claude";
  const id = accountIdFor(trimmed, accounts.map((account) => account.id));
  return [...accounts, { id, name: trimmed, configDir: configDir.trim() || null }];
}

/** Why the list cannot be saved as it is, or null. */
export function accountsProblem(accounts: readonly ClaudeAccount[]): string | null {
  if (accounts.length === 0) return "Keep at least one account.";
  if (accounts.length > MAX_ACCOUNTS) return `At most ${MAX_ACCOUNTS} accounts.`;
  if (accounts.some((account) => !account.name.trim())) return "Every account needs a name.";
  return null;
}
