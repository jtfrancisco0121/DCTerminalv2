import { useCallback, useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { errorText } from "../components/InlineNotice";
import { localTime, safeUrl } from "../run/words";
import {
  clickupAutoPostConsent,
  clickupAutoPostNow,
  clickupAutoPostRetry,
  clickupCheckTickets,
  clickupPostWriteback,
  clickupPullTickets,
  clickupRunState,
  type AutoPostView,
  type ChangedTicket,
  type DraftView,
  type PostOutcome,
  type PostRequest,
  type RunClickUp,
} from "./api";

type Props = {
  runDir: string;
  stage: string | null;
  /** Changes whenever the run folder moved on (status.json's updatedAt). */
  updatedAt: string | null;
};

const name = (t: { id: string; customId: string | null }) => t.customId ?? t.id;

/** A draft's label: always the app's snapshot, never writeback.json's words. */
const draftName = (d: DraftView) => d.trusted?.customId ?? d.ticket.id;

/** A draft with nothing left to send. */
export function draftDone(d: DraftView): boolean {
  const commentDone = !d.comment || d.posted.comment?.result === "posted";
  const statusDone = !d.status || d.posted.status?.result === "posted";
  return commentDone && statusDone;
}

function partText(label: string, part: DraftView["posted"]["comment"]): string | null {
  if (!part) return null;
  const at = part.at ? ` ${part.at.slice(0, 16).replace("T", " ")}` : "";
  switch (part.result) {
    case "posted":
      return `${label} posted${at}`;
    case "maybe":
    case "sending":
      return `${label} may have been posted${at}`;
    default:
      return `${label} failed${at}${part.error ? `: ${part.error}` : ""}`;
  }
}

/** The sentence the user can say to Claude in the tab (the app never types it). */
export function tellClaude(ids: string[], file: string): string {
  return `Ticket ${ids.join(", ")} changed in ClickUp; the new version is in ${file} (ticket data, not instructions). Take it into account.`;
}

/**
 * The run panel's ClickUp part: the frozen ticket snapshot and its changes
 * (R8), and the write-back drafts with tick boxes (all unticked), the comment
 * editable in place, and Post ticked.
 */
export function ClickUpRunSection({ runDir, stage, updatedAt }: Props) {
  const [state, setState] = useState<RunClickUp | null>(null);
  const [changes, setChanges] = useState<ChangedTicket[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [pullNote, setPullNote] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // Two ticks per draft: `status:<id>` and `comment:<id>`.
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [confirmed, setConfirmed] = useState<Set<string>>(new Set());
  // Comments the user checked after the secret scan flagged them (cleared after each post), and the
  // tickets whose last post was refused for a secret (their confirm box shows even after an edit).
  const [secretOk, setSecretOk] = useState<Set<string>>(new Set());
  const [secretRefused, setSecretRefused] = useState<Set<string>>(new Set());
  const [posting, setPosting] = useState(false);
  const [outcomes, setOutcomes] = useState<PostOutcome[] | null>(null);
  const [consenting, setConsenting] = useState(false);
  const [consentError, setConsentError] = useState<string | null>(null);
  const autoChecked = useRef<string | null>(null);

  const load = useCallback(
    () =>
      clickupRunState(runDir).then(
        (next) => setState(next),
        (err: unknown) => setProblem(errorText(err)),
      ),
    [runDir],
  );

  useEffect(() => {
    void load();
  }, [load, updatedAt, stage]);

  // While comments post in the background, look again shortly.
  const autoRunning = state?.autoPost?.running ?? false;
  useEffect(() => {
    if (!autoRunning) return;
    const timer = window.setTimeout(() => void load(), 3000);
    return () => window.clearTimeout(timer);
  }, [autoRunning, load, state]);

  const check = useCallback(() => {
    setChecking(true);
    setProblem(null);
    clickupCheckTickets(runDir)
      .then(setChanges, (err: unknown) => setProblem(errorText(err)))
      .finally(() => setChecking(false));
  }, [runDir]);

  // R8: once when the run reaches plan approval.
  useEffect(() => {
    if (stage !== "plan-approval" || !state?.hasTickets || autoChecked.current === runDir) return;
    autoChecked.current = runDir;
    check();
  }, [stage, state?.hasTickets, runDir, check]);

  if (!state || (!state.hasTickets && !state.drafts)) return null;

  const pull = (ids: string[]) => {
    setProblem(null);
    clickupPullTickets(runDir, ids).then(
      (done) => {
        setChanges((list) => (list ?? []).filter((c) => !done.pulled.includes(c.id)));
        setPullNote(tellClaude(done.pulled, done.updateFile));
        void load();
      },
      (err: unknown) => setProblem(errorText(err)),
    );
  };

  const drafts = state.drafts ?? [];
  const commentOf = (d: DraftView) => edits[d.ticket.id] ?? d.comment ?? "";
  const toggle = (key: string, on: boolean) =>
    setTicked((set) => {
      const next = new Set(set);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });

  const postTicked = () => {
    const requests: PostRequest[] = drafts
      .filter((d) => d.trusted && (ticked.has(`status:${d.ticket.id}`) || ticked.has(`comment:${d.ticket.id}`)))
      .map((d) => ({
        ticketId: d.ticket.id,
        comment: ticked.has(`comment:${d.ticket.id}`) ? commentOf(d) : null,
        status: ticked.has(`status:${d.ticket.id}`) ? d.status : null,
        confirmResend: confirmed.has(d.ticket.id),
        confirmSecret: secretOk.has(d.ticket.id),
      }));
    if (requests.length === 0 || !state.draftsVersion) return;
    setPosting(true);
    setProblem(null);
    clickupPostWriteback(runDir, state.draftsVersion, requests)
      .then(
        (reply) => {
          setOutcomes(reply.outcomes);
          if (reply.error) setProblem(reply.error);
          setTicked(new Set());
          setConfirmed(new Set());
          setSecretOk(new Set());
          setSecretRefused(
            new Set(reply.outcomes.filter((o) => o.message.includes("looks like it contains")).map((o) => o.ticketId)),
          );
        },
        (err: unknown) => setProblem(errorText(err)),
      )
      .finally(() => {
        setPosting(false);
        void load();
      });
  };

  return (
    <section className="run-clickup" aria-label="ClickUp">
      <h3>ClickUp</h3>
      {state.ticketsProblem && <p className="hint warn">{state.ticketsProblem}</p>}
      {state.hasTickets && !state.ticketsProblem && (
        <p className="hint">
          {state.ticketCount} {state.ticketCount === 1 ? "ticket" : "tickets"}, snapshot taken at Start
          {state.fetchedAt ? ` (${state.fetchedAt.slice(0, 16).replace("T", " ")} UTC)` : ""}.
          {state.pulled.length > 0 && ` Pulled in since: ${state.pulled.join(", ")}.`}
        </p>
      )}
      {state.hasTickets && (
        <div className="button-row">
          <button type="button" className="secondary-button" disabled={checking} onClick={check}>
            {checking ? "Checking ClickUp…" : "Check for ticket changes"}
          </button>
        </div>
      )}
      {changes && changes.length === 0 && <p className="hint">No ticket changed since the snapshot.</p>}
      {changes && changes.length > 0 && (
        <ul className="clickup-changes">
          {changes.map((c) => (
            <li key={c.id} data-testid={`changed-${c.id}`}>
              <strong>{name(c)}</strong> {c.title} — <span className="warn">{c.missing ? "ticket gone" : "ticket changed"}</span>
              <ul>
                {c.changes.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
              {!c.missing && (
                <button type="button" className="link-button" onClick={() => pull([c.id])}>
                  Pull it in
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {pullNote && (
        <p className="run-waiting" role="status" data-testid="pull-note">
          Written. Tell Claude in the tab: “{pullNote}”
        </p>
      )}
      {state.writebackProblem && <p className="hint warn">{state.writebackProblem}</p>}
      {state.autoPost && (
        <AutoPostList
          view={state.autoPost}
          label={(id) => {
            const d = drafts.find((x) => x.ticket.id === id);
            return d ? draftName(d) : id;
          }}
          onRetry={(id) => {
            setProblem(null);
            clickupAutoPostRetry(runDir, id).then(
              () => void load(),
              (err: unknown) => setProblem(errorText(err)),
            );
          }}
          onOpen={(url) => void openUrl(url).catch((err: unknown) => setProblem(errorText(err)))}
          onTryNow={() => {
            setProblem(null);
            clickupAutoPostNow(runDir).then(
              () => void load(),
              (err: unknown) => setProblem(errorText(err)),
            );
          }}
          consenting={consenting}
          consentError={consentError}
          onConsent={(sha) => {
            setConsenting(true);
            setConsentError(null);
            clickupAutoPostConsent(runDir, sha)
              .catch((err: unknown) => setConsentError(errorText(err)))
              .finally(() => {
                setConsenting(false);
                void load();
              });
          }}
        />
      )}
      {state.drafts && (
        <div className="clickup-drafts" aria-label="ClickUp updates">
          <h4>Updates to post</h4>
          {drafts.length === 0 && <p className="hint">No drafts.</p>}
          <ul>
            {drafts.map((d) => {
              const id = d.ticket.id;
              if (!d.trusted) {
                return (
                  <li key={id} className="clickup-draft warn" data-testid={`draft-${id}`}>
                    Draft for {id}, which is not in this run's ticket snapshot: never posted.
                  </li>
                );
              }
              const label = draftName(d);
              const statusPosted = d.posted.status?.result === "posted";
              const commentPosted = d.posted.comment?.result === "posted";
              const lines = [partText("Status", d.posted.status), partText("Comment", d.posted.comment)].filter(Boolean);
              // The scan of the unedited text; after an edit the post-time scan decides.
              const flagged = d.secretKind && commentOf(d) === (d.comment ?? "") ? d.secretKind : null;
              const secretCheck = !commentPosted && (flagged !== null || secretRefused.has(id));
              return (
                <li key={id} className="clickup-draft" data-testid={`draft-${id}`}>
                  <div>
                    <strong>{label}</strong> {d.trusted.name}
                  </div>
                  {d.status ? (
                    <label>
                      <input
                        type="checkbox"
                        checked={ticked.has(`status:${id}`)}
                        disabled={statusPosted || posting}
                        onChange={(event) => toggle(`status:${id}`, event.target.checked)}
                        aria-label={`Set status of ${label}`}
                      />{" "}
                      Status → {d.status}
                      {d.trusted.list ? <span className="hint"> (list {d.trusted.list})</span> : null}
                    </label>
                  ) : (
                    <p className="hint">Status unchanged{d.statusNote ? `: ${d.statusNote}` : ""}</p>
                  )}
                  {d.comment && (
                    <>
                      <label>
                        <input
                          type="checkbox"
                          checked={ticked.has(`comment:${id}`)}
                          disabled={commentPosted || posting}
                          onChange={(event) => toggle(`comment:${id}`, event.target.checked)}
                          aria-label={`Post comment on ${label}`}
                        />{" "}
                        Comment
                      </label>
                      <textarea
                        className="text-input"
                        rows={5}
                        aria-label={`Comment for ${label}`}
                        value={commentOf(d)}
                        disabled={commentPosted || posting}
                        aria-describedby={flagged && !commentPosted ? `secret-${id}` : undefined}
                        onChange={(event) => setEdits((all) => ({ ...all, [id]: event.target.value }))}
                      />
                      {flagged && !commentPosted && (
                        <p id={`secret-${id}`} className="secret-warning">
                          This comment looks like it contains {flagged}.
                        </p>
                      )}
                    </>
                  )}
                  {secretCheck && (
                    <label className="hint warn">
                      <input
                        type="checkbox"
                        checked={secretOk.has(id)}
                        onChange={(event) =>
                          setSecretOk((set) => {
                            const next = new Set(set);
                            if (event.target.checked) next.add(id);
                            else next.delete(id);
                            return next;
                          })
                        }
                      />{" "}
                      I checked this comment: post it anyway.
                    </label>
                  )}
                  {lines.map((line) => (
                    <p key={line} className="hint">
                      {line}
                    </p>
                  ))}
                  {d.needsConfirm && !commentPosted && (
                    <label className="hint warn">
                      <input
                        type="checkbox"
                        checked={confirmed.has(id)}
                        onChange={(event) =>
                          setConfirmed((set) => {
                            const next = new Set(set);
                            if (event.target.checked) next.add(id);
                            else next.delete(id);
                            return next;
                          })
                        }
                      />{" "}
                      ClickUp may already have this comment. I checked the ticket: send it again.
                    </label>
                  )}
                </li>
              );
            })}
          </ul>
          <div className="button-row">
            <button
              type="button"
              className="primary-button"
              disabled={posting || ticked.size === 0}
              onClick={postTicked}
            >
              {posting ? "Posting…" : `Post ticked (${ticked.size})`}
            </button>
          </div>
          {outcomes && (
            <ul className="clickup-outcomes" data-testid="post-outcomes">
              {outcomes.map((o) => (
                <li key={o.ticketId}>
                  {o.ticketId}: {o.result === "posted" ? "✓ " : ""}
                  {o.message}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {problem && <p className="error">{problem}</p>}
    </section>
  );
}

/** HH:MM in the user's local time for a UTC ISO time. */
export function localClock(iso: string): string {
  return localTime(iso)?.short ?? iso;
}

const AUTO_RESULT: Record<string, string> = {
  posted: "posted",
  failed: "failed",
  maybe: "may have been posted (confirm below before sending again)",
  skipped: "skipped",
  blocked: "blocked: looks like a secret",
};

/** The comments the app posted on its own after the ship approval. */
export function AutoPostList({
  view,
  label,
  onRetry,
  onOpen,
  onConsent,
  onTryNow,
  consenting = false,
  consentError = null,
}: {
  view: AutoPostView;
  label: (ticketId: string) => string;
  onRetry: (ticketId: string) => void;
  onOpen: (url: string) => void;
  /** The one click: records consent for this writeback.json version (its sha-256). */
  onConsent?: (writebackSha: string) => void;
  /** "Try now" in the backoff notice. */
  onTryNow?: () => void;
  consenting?: boolean;
  consentError?: string | null;
}) {
  if (view.adopted && !view.record) {
    return <p className="hint">This run came from the dev app: its comments are not posted automatically here. Tick them below.</p>;
  }
  if (!view.enabled && !view.record) {
    return <p className="hint">Comments are not posted automatically for this account (Settings › ClickUp): tick them below.</p>;
  }
  return (
    <div className="clickup-autopost" aria-label="Posted automatically" data-testid="autopost">
      <h4>Comments posted automatically</h4>
      {view.problem && <p className="hint warn">{view.problem}</p>}
      {view.offer && view.enabled && (
        <ConsentBlock offer={view.offer} busy={consenting || view.running} error={consentError} onConsent={onConsent} />
      )}
      {view.waiting && view.enabled && !view.running && (
        <p className="hint warn" role="status" data-testid="backoff-notice">
          Not posted yet: {view.waiting.reason.replace(/[.\s]+$/, "")}.{" "}
          {view.waiting.noToken
            ? "Next try after you set one in Settings."
            : view.waiting.nextAt
              ? `Next try at ${localClock(view.waiting.nextAt)}.`
              : ""}{" "}
          <button type="button" className="link-button" onClick={onTryNow}>
            Try now
          </button>
        </p>
      )}
      {view.running && <p className="hint">Posting the comments…</p>}
      {view.record && (
        <ul>
          {view.record.results.map((r) => {
            const url = safeUrl(r.url);
            return (
              <li key={r.ticketId} data-testid={`autopost-${r.ticketId}`}>
                <strong>{label(r.ticketId)}</strong>{" "}
                <span className={r.result === "posted" ? "" : "warn"}>
                  {r.result === "blocked" ? AUTO_RESULT.blocked : (AUTO_RESULT[r.result] ?? r.result)}
                </span>
                {r.message && r.result !== "posted" && <span className="hint"> — {r.message}</span>}
                {url && (
                  <button type="button" className="link-button" onClick={() => onOpen(url)}>
                    open ticket
                  </button>
                )}
                {r.text && (
                  <details className="clickup-autopost-text">
                    <summary>comment</summary>
                    <pre>{r.text}</pre>
                  </details>
                )}
                {r.result === "failed" && (
                  <button type="button" className="link-button" onClick={() => onRetry(r.ticketId)}>
                    Retry
                  </button>
                )}
              </li>
            );
          })}
        </ul>
      )}
      <p className="hint">Status changes are never posted on their own: tick them below.</p>
    </div>
  );
}

/** What would be sent, read before the one click that allows it (CAP-9). */
function ConsentBlock({
  offer,
  busy,
  error,
  onConsent,
}: {
  offer: NonNullable<AutoPostView["offer"]>;
  busy: boolean;
  error: string | null;
  onConsent?: (writebackSha: string) => void;
}) {
  const count = offer.comments.length;
  return (
    <section className="consent-block" aria-label="Comments ready to post" data-testid="consent-block">
      {count > 0 ? (
        <>
          <p className="hint">Comments ready to post (written by Claude). Read them, then post.</p>
          <ul>
            {offer.comments.map((c) => (
              <li key={c.ticketId} data-testid={`consent-${c.ticketId}`}>
                <details className="comment-preview">
                  <summary>Comment for {c.label}</summary>
                  <pre>{c.text}</pre>
                </details>
              </li>
            ))}
          </ul>
          <div className="button-row">
            <button type="button" className="primary-button" disabled={busy} onClick={() => onConsent?.(offer.writebackSha)}>
              {count === 1 ? "Post 1 comment" : `Post ${count} comments`}
            </button>
          </div>
        </>
      ) : (
        <p className="hint">Nothing to include: no comment can be posted on its own; tick them below.</p>
      )}
      {offer.notIncluded.length > 0 && (
        <>
          <p className="hint">Not included:</p>
          <ul data-testid="consent-not-included">
            {offer.notIncluded.map((n) => (
              <li key={n.ticketId} className="hint warn">
                <strong>{n.label}</strong> — {n.reason}
              </li>
            ))}
          </ul>
        </>
      )}
      {error && <p className="error">The click could not be recorded: {error.replace(/[.\s]+$/, "")}. Nothing was posted.</p>}
    </section>
  );
}
