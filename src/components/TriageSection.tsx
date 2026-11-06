import { useCallback, useEffect, useState } from "react";
import {
  triageApply,
  triageApprove,
  triageOverview,
  type TriageEdits,
  type TriageGroup,
  type TriageView,
} from "../clickup/api";
import { errorText } from "./InlineNotice";

const POLL_MS = 5000;

/** One proposed group as the operator is editing it. `id` is the proposal's own number; the order is the array's. */
export type Card = TriageGroup & { id: number; keep: boolean };

export function cardsOf(view: TriageView): Card[] {
  return (view.proposal?.groups ?? []).map((g) => ({ ...g, id: g.n, keep: true }));
}

/** Why the cards as edited cannot be approved, or null. An empty group and a dependency on a later group are refused. */
export function cardsProblem(cards: Card[]): string | null {
  const kept = cards.filter((c) => c.keep);
  const position = new Map(kept.map((c, at) => [c.id, at]));
  for (const [at, card] of kept.entries()) {
    if (card.tickets.length === 0) return `Group ${at + 1} (${card.title}) would be empty: move a ticket back or reject the group.`;
    for (const dep of card.dependsOn) {
      const depAt = position.get(dep);
      if (depAt === undefined) continue; // a rejected group is no dependency any more
      if (depAt >= at) {
        return `Group ${at + 1} (${card.title}) depends on group ${depAt + 1} (${kept[depAt].title}), which would come after it.`;
      }
    }
  }
  return null;
}

/** The groups kept, numbered by position, with dependencies renumbered to match. */
export function editsOf(cards: Card[], creates: Set<number>, closes: Set<string>): TriageEdits {
  const kept = cards.filter((c) => c.keep);
  const position = new Map(kept.map((c, at) => [c.id, at + 1]));
  return {
    groups: kept.map((c) => ({
      title: c.title,
      tickets: c.tickets,
      dependsOn: c.dependsOn.map((d) => position.get(d)).filter((n): n is number => n !== undefined),
      why: c.why,
    })),
    creates: [...creates].sort((a, b) => a - b),
    closes: [...closes],
  };
}

export function summaryLine(cards: Card[], creates: Set<number>, closes: Set<string>): string {
  const groups = cards.filter((c) => c.keep).length;
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  return `Approving queues ${plural(groups, "group", "groups")}, creates ${plural(creates.size, "ticket", "tickets")} and closes ${plural(closes.size, "ticket", "tickets")} in ClickUp.`;
}

const WRITE_WORDS: Record<string, string> = {
  posted: "sent",
  sending: "sending…",
  maybe: "may have been sent: check ClickUp, it is not sent again",
  failed: "failed",
  blocked: "not sent: looks like a secret",
};

function Proposal({ view, onChanged }: { view: TriageView; onChanged: () => void }) {
  const proposal = view.proposal!;
  const [cards, setCards] = useState<Card[]>(() => cardsOf(view));
  const [creates, setCreates] = useState<Set<number>>(new Set());
  const [closes, setCloses] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const name = (id: string) => proposal.names?.[id] ?? id;
  const problem = cardsProblem(cards);

  const patch = (id: number, change: Partial<Card>) => setCards((all) => all.map((c) => (c.id === id ? { ...c, ...change } : c)));
  const move = (at: number, by: number) =>
    setCards((all) => {
      const next = [...all];
      const to = at + by;
      if (to < 0 || to >= next.length) return all;
      [next[at], next[to]] = [next[to], next[at]];
      return next;
    });
  const moveTicket = (ticket: string, from: number, to: number) =>
    setCards((all) =>
      all.map((c) =>
        c.id === from ? { ...c, tickets: c.tickets.filter((t) => t !== ticket) } : c.id === to ? { ...c, tickets: [...c.tickets, ticket] } : c,
      ),
    );
  const toggle = <T,>(set: Set<T>, value: T, on: boolean) => {
    const next = new Set(set);
    if (on) next.add(value);
    else next.delete(value);
    return next;
  };

  const decide = async (decision: "approved" | "rejected") => {
    setBusy(true);
    setError(null);
    try {
      const record = await triageApprove(view.runDir, view.digest, decision, decision === "approved" ? editsOf(cards, creates, closes) : null);
      if (record.creates.length > 0 || record.closes.length > 0) await triageApply(view.runDir);
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
      onChanged();
    }
  };

  return (
    <div className="triage-proposal" data-testid={`triage-${view.runId}`}>
      <ul className="triage-groups">
        {cards.map((card, at) => (
          <li key={card.id} className={`triage-group-card${card.keep ? "" : " triage-group-rejected"}`} data-testid={`triage-group-${card.id}`}>
            <div className="triage-group-head">
              <strong>
                Group {cards.filter((c, i) => c.keep && i <= at).length}: {card.title}
              </strong>
              <label className="check-label">
                <input
                  type="checkbox"
                  aria-label={`Keep group ${card.title}`}
                  checked={card.keep}
                  onChange={(event) => patch(card.id, { keep: event.target.checked })}
                />{" "}
                Keep
              </label>
              <button type="button" className="link-button" aria-label={`Move ${card.title} up`} disabled={at === 0} onClick={() => move(at, -1)}>
                Up
              </button>
              <button type="button" className="link-button" aria-label={`Move ${card.title} down`} disabled={at === cards.length - 1} onClick={() => move(at, 1)}>
                Down
              </button>
            </div>
            {card.why && <p className="hint">{card.why}</p>}
            <ul>
              {card.tickets.map((ticket) => (
                <li key={ticket}>
                  {name(ticket)} <span className="hint">({ticket})</span>{" "}
                  <select
                    className="text-input text-input-narrow"
                    aria-label={`Move ${ticket} to`}
                    value=""
                    onChange={(event) => event.target.value && moveTicket(ticket, card.id, Number(event.target.value))}
                  >
                    <option value="">Move to…</option>
                    {cards
                      .filter((c) => c.id !== card.id)
                      .map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.title}
                        </option>
                      ))}
                  </select>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
      {proposal.needsYou.length > 0 && (
        <div className="triage-needs">
          <h4>Needs you</h4>
          {proposal.needsYou.map((n) => (
            <span key={n.ticket} className="triage-flag-chip" title={n.reason}>
              {name(n.ticket)}: {n.reason}
            </span>
          ))}
        </div>
      )}
      {(proposal.suggestions.length > 0 || proposal.closes.length > 0) && (
        <div className="triage-suggestions">
          <h4>Suggestions (unticked: nothing is sent unless you tick it)</h4>
          {proposal.suggestions.map((s) => (
            <label key={`c${s.index}`} className="check-label triage-suggestion-row">
              <input
                type="checkbox"
                aria-label={`Create ticket ${s.title}`}
                checked={creates.has(s.index)}
                onChange={(event) => setCreates((c) => toggle(c, s.index, event.target.checked))}
              />{" "}
              Create “{s.title}” <span className="hint">{s.why}</span>
            </label>
          ))}
          {proposal.closes.map((c) => (
            <label key={`x${c.ticket}`} className="check-label triage-suggestion-row">
              <input
                type="checkbox"
                aria-label={`Close ticket ${c.ticket}`}
                checked={closes.has(c.ticket)}
                onChange={(event) => setCloses((s) => toggle(s, c.ticket, event.target.checked))}
              />{" "}
              Close {name(c.ticket)} <span className="hint">{c.why}</span>
            </label>
          ))}
        </div>
      )}
      <div className="triage-approve-bar">
        <p className="hint" data-testid="triage-summary">
          {summaryLine(cards, creates, closes)}
        </p>
        {problem && (
          <p className="error" role="alert" data-testid="triage-refusal">
            {problem}
          </p>
        )}
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        <div className="button-row">
          <button type="button" className="primary-button" disabled={busy || Boolean(problem)} onClick={() => void decide("approved")}>
            Approve
          </button>
          <button type="button" className="secondary-button" disabled={busy} onClick={() => void decide("rejected")}>
            Reject
          </button>
        </div>
      </div>
    </div>
  );
}

function Decided({ view, onChanged }: { view: TriageView; onChanged: () => void }) {
  const approval = view.approval!;
  const [error, setError] = useState<string | null>(null);
  const items = Object.entries(view.posted?.items ?? {});
  const retry = async () => {
    setError(null);
    try {
      await triageApply(view.runDir);
    } catch (err) {
      setError(errorText(err));
    }
    onChanged();
  };
  if (approval.decision === "rejected") {
    return <p className="hint">Rejected on {approval.at.slice(0, 10)}. Nothing was started.</p>;
  }
  return (
    <div data-testid={`triage-approved-${view.runId}`}>
      <p className="hint">
        Approved on {approval.at.slice(0, 10)}: {approval.groups.length} {approval.groups.length === 1 ? "group" : "groups"} queued, {approval.creates.length}{" "}
        to create, {approval.closes.length} to close.
      </p>
      <ol>
        {approval.groups.map((g) => (
          <li key={g.n}>
            Group {g.n}: {g.title} ({g.tickets.length} tickets)
          </li>
        ))}
      </ol>
      {items.length > 0 && (
        <ul className="triage-writes">
          {items.map(([item, post]) => (
            <li key={item} data-testid={`triage-write-${item}`}>
              {item.startsWith("create:") ? `Create #${Number(item.slice(7)) + 1}` : `Close ${item.slice(6)}`}: {WRITE_WORDS[post.result] ?? post.result}
              {post.result === "failed" && post.message ? ` (${post.message})` : ""}
              {post.result === "failed" && (
                <button type="button" className="link-button" onClick={() => void retry()}>
                  Retry
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {error && <p className="error">{error}</p>}
    </div>
  );
}

/** The Runs view's Triage: a finished triage run's proposal to approve or reject, then what it queued and sent. */
export function TriageSection() {
  const [views, setViews] = useState<TriageView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(
    () =>
      triageOverview().then(
        (v) => setViews(v),
        (err: unknown) => setError(errorText(err)),
      ),
    [],
  );
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);
  if (error) return <p className="error">{error}</p>;
  if (!views || views.length === 0) return null;
  return (
    <section className="runs-triage" aria-label="Triage" data-testid="triage">
      <h3>Triage</h3>
      {views.map((view) => (
        <div key={view.runDir}>
          <p className="hint">{view.runId}</p>
          {view.problem && !view.approval && (
            <p className="hint warn" role="status" data-testid={`triage-problem-${view.runId}`}>
              {view.problem}
            </p>
          )}
          {view.approval ? <Decided view={view} onChanged={() => void load()} /> : view.proposal && !view.problem && <Proposal key={view.digest} view={view} onChanged={() => void load()} />}
        </div>
      ))}
    </section>
  );
}
