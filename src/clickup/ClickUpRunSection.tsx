import { useCallback, useEffect, useRef, useState } from "react";
import { errorText } from "../components/InlineNotice";
import {
  clickupCheckTickets,
  clickupPostWriteback,
  clickupPullTickets,
  clickupRunState,
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
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [confirmed, setConfirmed] = useState<Set<string>>(new Set());
  const [posting, setPosting] = useState(false);
  const [outcomes, setOutcomes] = useState<PostOutcome[] | null>(null);
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
  const toggle = (id: string, on: boolean) =>
    setTicked((set) => {
      const next = new Set(set);
      if (on) next.add(id);
      else next.delete(id);
      return next;
    });

  const postTicked = () => {
    const requests: PostRequest[] = drafts
      .filter((d) => ticked.has(d.ticket.id))
      .map((d) => ({
        ticketId: d.ticket.id,
        comment: d.comment && d.posted.comment?.result !== "posted" ? commentOf(d) : null,
        status: !!d.status && d.posted.status?.result !== "posted",
        confirmResend: confirmed.has(d.ticket.id),
      }));
    if (requests.length === 0) return;
    setPosting(true);
    setProblem(null);
    clickupPostWriteback(runDir, requests)
      .then(
        (reply) => {
          setOutcomes(reply.outcomes);
          if (reply.error) setProblem(reply.error);
          setTicked(new Set());
          setConfirmed(new Set());
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
      {state.drafts && (
        <div className="clickup-drafts" aria-label="ClickUp updates">
          <h4>Updates to post</h4>
          {drafts.length === 0 && <p className="hint">No drafts.</p>}
          <ul>
            {drafts.map((d) => {
              const id = d.ticket.id;
              const done = draftDone(d);
              const commentPosted = d.posted.comment?.result === "posted";
              const lines = [partText("Status", d.posted.status), partText("Comment", d.posted.comment)].filter(Boolean);
              return (
                <li key={id} className="clickup-draft" data-testid={`draft-${id}`}>
                  <label>
                    <input
                      type="checkbox"
                      checked={ticked.has(id)}
                      disabled={done || !d.inTickets || posting}
                      onChange={(event) => toggle(id, event.target.checked)}
                      aria-label={`Post ${name(d.ticket)}`}
                    />{" "}
                    <strong>{name(d.ticket)}</strong> {d.ticket.title ?? ""}
                  </label>
                  {!d.inTickets && <p className="hint warn">Not in this run's tickets.json: never posted.</p>}
                  <p className="hint">
                    {d.status ? `Status → ${d.status}` : `Status unchanged${d.statusNote ? `: ${d.statusNote}` : ""}`}
                  </p>
                  {d.comment && (
                    <textarea
                      className="text-input"
                      rows={5}
                      aria-label={`Comment for ${name(d.ticket)}`}
                      value={commentOf(d)}
                      disabled={commentPosted || posting}
                      onChange={(event) => setEdits((all) => ({ ...all, [id]: event.target.value }))}
                    />
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
