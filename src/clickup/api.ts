/**
 * Typed wrappers for the ClickUp commands (src-tauri/src/clickup/commands.rs):
 * the token per account, Fetch, and the run panel's ticket changes (R8) and
 * write-back. No reply ever carries the token.
 */

import { invoke } from "@tauri-apps/api/core";
import type { IntakeRef } from "./intakeRefs";

export type TokenStatus = { accountId: string; saved: boolean; problem: string | null };
export type TokenTest = { workspaces: string[] };

export type TicketRow = {
  id: string;
  customId: string | null;
  title: string;
  status: string;
  list: string;
  url: string;
};
export type ExcludedTicket = TicketRow & { reason: string };
export type IntakePreview = {
  tickets: TicketRow[];
  excluded: ExcludedTicket[];
  problems: { line: number; message: string }[];
  /** Over 100 tickets: shown, but Start refuses until some are removed. */
  tooMany: string | null;
};

/** What Start passes to `start_run` for Eagle-Eye. */
export type ClickUpStart = { refs: IntakeRef[]; ticketIds: string[] };

export type PartResult = {
  /** `posted`, `failed`, `maybe` (may have been posted), `sending` (stopped mid-call). */
  result: string;
  at: string;
  value: string;
  error?: string | null;
};

export type DraftView = {
  ticket: { id: string; customId: string | null; title: string | null; url: string | null };
  prs: { id: string | null; repo: string | null; url: string | null }[];
  comment: string | null;
  status: string | null;
  statusNote: string | null;
  posted: { comment?: PartResult; status?: PartResult };
  needsConfirm: boolean;
  /** What the app's secret scan finds in the unedited comment ("a GitHub token"), if anything. */
  secretKind?: string | null;
  /** From the app's own snapshot; null: not in it, never posted. */
  trusted: { customId: string | null; name: string; url: string; list: string | null } | null;
};

export type RunClickUp = {
  hasTickets: boolean;
  ticketCount: number;
  fetchedAt: string | null;
  ticketsProblem: string | null;
  pulled: string[];
  updateFile: string | null;
  drafts: DraftView[] | null;
  /** writeback.json's version as shown; posting with another is refused. */
  draftsVersion: string | null;
  writebackProblem: string | null;
  /** Comments posted on their own after the ship approval (per-account setting). */
  autoPost?: AutoPostView | null;
};

export type AutoPostResult = {
  ticketId: string;
  /** `posted`, `failed` (retry), `maybe` (confirm in the ticks), `skipped`, `blocked` (looks like a secret). */
  result: string;
  message: string;
  url: string | null;
  at: string;
  /** The comment as posted. */
  text?: string | null;
};
export type AutoPostView = {
  enabled: boolean;
  running: boolean;
  record: { schema: number; writebackSha: string; at: string; results: AutoPostResult[] } | null;
  problem: string | null;
  /** A dev-app run used in place: only the app that owns it posts on its own. */
  adopted?: boolean;
  /** Waiting for the user's click: what would go, and what never goes on its own. */
  offer?: ConsentOffer | null;
};
export type ConsentOffer = {
  /** The writeback.json version shown; the click covers exactly this. */
  writebackSha: string;
  comments: { ticketId: string; label: string; text: string }[];
  notIncluded: { ticketId: string; label: string; reason: string }[];
};

export type ChangedTicket = {
  id: string;
  customId: string | null;
  title: string;
  missing: boolean;
  dateUpdated: string | null;
  changes: string[];
};

export type PostRequest = {
  ticketId: string;
  /** The comment as shown (edited or not); null: not ticked. */
  comment: string | null;
  /** The status as shown (the draft's); null: not ticked. */
  status: string | null;
  confirmResend: boolean;
  /** This comment was flagged by the secret scan and you checked it: send it anyway. */
  confirmSecret: boolean;
};
export type PostOutcome = { ticketId: string; result: string; message: string };
export type PostReply = { outcomes: PostOutcome[]; error: string | null };

export const clickupTokenStatus = (accountId: string) => invoke<TokenStatus>("clickup_token_status", { accountId });
export const clickupSetToken = (accountId: string, token: string) =>
  invoke<void>("clickup_set_token", { accountId, token });
export const clickupClearToken = (accountId: string) => invoke<void>("clickup_clear_token", { accountId });
export const clickupTestToken = (accountId: string) => invoke<TokenTest>("clickup_test_token", { accountId });
export const clickupFetch = (windowId: string, refs: IntakeRef[]) =>
  invoke<IntakePreview>("clickup_fetch", { windowId, refs });
export const clickupRunState = (runDir: string) => invoke<RunClickUp>("clickup_run_state", { runDir });
export const clickupCheckTickets = (runDir: string) => invoke<ChangedTicket[]>("clickup_check_tickets", { runDir });
export const clickupPullTickets = (runDir: string, ids: string[]) =>
  invoke<{ pulled: string[]; updateFile: string }>("clickup_pull_tickets", { runDir, ids });
export const clickupPostWriteback = (runDir: string, draftsVersion: string, requests: PostRequest[]) =>
  invoke<PostReply>("clickup_post_writeback", { runDir, draftsVersion, requests });
export const clickupAutoPostRetry = (runDir: string, ticketId: string) =>
  invoke<NonNullable<AutoPostView["record"]>>("clickup_auto_post_retry", { runDir, ticketId });
/** The one click: records consent for the writeback.json version shown, then the app posts. */
export const clickupAutoPostConsent = (runDir: string, writebackSha: string) =>
  invoke<void>("clickup_auto_post_consent", { runDir, writebackSha });
export const clickupSetAutoPost = (accountId: string, on: boolean) =>
  invoke<void>("clickup_set_auto_post", { accountId, on });
