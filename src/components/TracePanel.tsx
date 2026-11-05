import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { flightContent, flightReplayFile, flightReplayTrack, flightSession } from "../flight/api";
import { ORCHESTRATION } from "../flight/types";
import type {
  AgentNode,
  ContentOut,
  Failure,
  FlightEvent,
  Phase,
  ReplayTrackOut,
  SessionView,
  SnapshotInfo,
  TraceTarget,
} from "../flight/types";
import {
  flightDerivedText,
  flightKindText,
  flightStatusText,
  flightStatusTone,
  localTime,
  orchestrationStatusText,
  orchestrationText,
  orchestrationTone,
  replayDisabledReason,
} from "../run/words";
import { errorText } from "./InlineNotice";
import { ExportDialog } from "./ExportDialog";
import { FilePane, type FileState } from "./FilePane";
import { Pill } from "./Pill";
import { ReplayTrack } from "./ReplayTrack";
import { RerunCheck } from "./RerunCheck";

/** At most this many pages are fetched to bring a failure's event into the list. */
const MAX_JUMP_PAGES = 50;
const PLACEHOLDER_ROWS = 6;

type Row =
  | { type: "event"; id: string; event: FlightEvent }
  | { type: "unknown"; id: string; count: number; events: FlightEvent[] };

type ContentState =
  | { id: string; state: "loading" }
  | { id: string; state: "ready"; out: ContentOut }
  | { id: string; state: "error"; message: string };

type Lane = "agents" | "orchestration" | "both";
const LANES: { id: Lane; label: string }[] = [
  { id: "agents", label: "Agents" },
  { id: "orchestration", label: "Orchestration" },
  { id: "both", label: "Both" },
];

const isRun = (event: FlightEvent) => event.agent.id === ORCHESTRATION;
const ruleOf = (event: FlightEvent) => (typeof event.attrs.rule === "string" ? event.attrs.rule : null);
const prOfAttrs = (event: FlightEvent) => (typeof event.attrs.pr === "string" ? event.attrs.pr : null);

/** Agent id -> PR id, from the agent trees. */
function prsOf(nodes: AgentNode[], out = new Map<string, string>()): Map<string, string> {
  for (const node of nodes) {
    if (node.pr) out.set(node.id, node.pr);
    prsOf(node.children, out);
  }
  return out;
}

/** "PR2": solid when the tie is exact; dashed "PR2 · heuristic" with a tooltip (hover and focus) when it was guessed. */
/** The host's refusal for a gone transcript whose snapshot kept no content for the line. */
const CONTENT_NOT_KEPT = "content not kept";

/** Says the Events of a gone session came from the run's snapshot; the tooltip has its time and whether content was kept. */
function SnapshotPill({ info }: { info: SnapshotInfo }) {
  const when = localTime(info.createdAt);
  const kept = info.contentKept ? "content kept" : "content not kept";
  return (
    <Pill
      tone="muted"
      className="pill-snapshot"
      testId="trace-snapshot"
      title={`Snapshot of ${when?.title ?? info.createdAt} (policy ${info.policy}, ${kept})`}
    >
      Snapshot
    </Pill>
  );
}

function PrPill({ pr, label }: { pr: string; label: string | undefined }) {
  if (label?.startsWith("exact:")) {
    return (
      <Pill tone="muted" className="pill-pr" title={label}>
        {pr}
      </Pill>
    );
  }
  const tip = "Guessed from the agent's description: this run is from a plugin before the exact PR rule.";
  return (
    <span className="pill tone-muted pill-pr pill-heuristic" tabIndex={0} data-tip={tip} title={label ?? tip} aria-label={`${pr}, heuristic. ${tip}`}>
      {pr} · heuristic
    </span>
  );
}

const clock = (at: string) => {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? "--:--:--" : date.toLocaleTimeString(undefined, { hourCycle: "h23" });
};

const agentName = (agent: { id: string; role: string | null; type: string | null }) =>
  agent.id === "main" ? "orchestrator" : (agent.role ?? agent.type ?? agent.id);

/** Consecutive unknown lines collapse into one row; an opened group shows its lines. */
function rowsOf(events: FlightEvent[], opened: ReadonlySet<string>): Row[] {
  const rows: Row[] = [];
  let run: FlightEvent[] = [];
  const flush = () => {
    if (run.length === 0) return;
    const id = `unknown-${run[0]!.id}`;
    if (run.length === 1 || opened.has(id)) {
      for (const event of run) rows.push({ type: "event", id: event.id, event });
    } else {
      rows.push({ type: "unknown", id, count: run.length, events: run });
    }
    run = [];
  };
  for (const event of events) {
    if (event.kind === "x.claude.unknown") run.push(event);
    else {
      flush();
      rows.push({ type: "event", id: event.id, event });
    }
  }
  flush();
  return rows;
}

function flatFailures(failures: Failure[]): Failure[] {
  return failures.flatMap((f) => [f, ...f.repeats]).sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
}

function AgentTree({
  nodes,
  agent,
  onPick,
}: {
  nodes: AgentNode[];
  agent: string | undefined;
  onPick: (id: string | undefined) => void;
}) {
  return (
    <ul className="trace-tree">
      {nodes.map((node) => {
        const name = agentName({ id: node.id, role: node.role, type: node.type });
        return (
          <li key={`${node.session}:${node.id}`}>
            <button
              type="button"
              className="trace-agent"
              aria-pressed={agent === node.id}
              onClick={() => onPick(agent === node.id ? undefined : node.id)}
            >
              {name}
              {node.pr && (
                <>
                  {" "}
                  <PrPill pr={node.pr} label={node.prDerived?.label} />
                </>
              )}
              {node.files.length > 0 && <span className="hint"> · {node.files.length} files</span>}
            </button>
            {node.files.length > 0 && (
              <details className="trace-files">
                <summary>Files {name} touched</summary>
                <ul>
                  {node.files.map((path) => (
                    <li key={path}>{path}</li>
                  ))}
                </ul>
              </details>
            )}
            {node.children.length > 0 && <AgentTree nodes={node.children} agent={agent} onPick={onPick} />}
          </li>
        );
      })}
    </ul>
  );
}

/** The fields of an opened event: what it is, when, its attributes and the phase of the run it happened in. */
function EventFields({ event, phase }: { event: FlightEvent; phase: Phase | undefined }) {
  const attrs = Object.entries(event.attrs).filter(([, v]) => typeof v === "string" || typeof v === "number");
  return (
    <dl className="trace-fields" data-testid="trace-fields">
      <dt>Event</dt>
      <dd>
        {flightKindText(event.kind)}
        {event.name ? `: ${event.name}` : ""} · {event.at}
      </dd>
      {attrs.map(([k, v]) => (
        <div key={k} className="trace-field">
          <dt>{k}</dt>
          <dd>{String(v)}</dd>
        </div>
      ))}
      {phase && (phase.stage || phase.prState) && (
        <>
          <dt>Phase</dt>
          <dd>
            {phase.stage ? `stage ${phase.stage}` : ""}
            {phase.prState ? ` · PR ${phase.prState}${phase.label?.startsWith("heuristic:") ? " (heuristic)" : ""}` : ""}
          </dd>
        </>
      )}
    </dl>
  );
}

type Props = {
  target: TraceTarget;
  /** Open on this agent only (a lane click). */
  agent?: string;
  /** Open on the page holding this event (a search result's first match). */
  focus?: string;
  onClose: () => void;
  /** The Search button of the header (traces opened from the search page, or any run Trace). */
  onSearch?: () => void;
};

/**
 * The Session Inspector: which tool call failed, what came before it, whether it repeated, and which files each
 * agent touched, from the run's Claude transcripts. Content is read only when an event is opened and arrives
 * redacted; it is shown as plain text in a `pre`.
 */
export function TracePanel({ target, agent: initialAgent, focus, onClose, onSearch }: Props) {
  const [agent, setAgent] = useState<string | undefined>(initialAgent);
  const [view, setView] = useState<SessionView | null>(null);
  const [events, setEvents] = useState<FlightEvent[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [phase, setPhase] = useState<"cold" | "more" | null>("cold");
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [failureId, setFailureId] = useState<string | null>(null);
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());
  const [content, setContent] = useState<ContentState | null>(null);
  const [copied, setCopied] = useState(false);
  const [lane, setLane] = useState<Lane>("both");
  const [prFilter, setPrFilter] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [replay, setReplay] = useState(false);
  const [track, setTrack] = useState<ReplayTrackOut | null>(null);
  const [trackError, setTrackError] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const [fileState, setFileState] = useState<FileState | null>(null);
  const focusRef = useRef<string | undefined>(focus);
  const scrollTo = useRef<string | null>(null);
  const page = useRef<HTMLElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const serial = useRef(0);
  const targetKey = JSON.stringify(target);

  const load = useCallback(
    async (filter: string | undefined) => {
      const mine = ++serial.current;
      setPhase("cold");
      try {
        const at = focusRef.current;
        const v = await flightSession({ ...target, agent: filter, ...(at ? { focus: at } : {}) });
        if (mine !== serial.current) return;
        setView(v);
        setEvents(v.page.events);
        setNext(v.page.nextCursor);
        setError(null);
        if (at) {
          // Open on the focused event once, not again on a refresh or an agent filter.
          focusRef.current = undefined;
          if (v.page.events.some((e) => e.id === at)) {
            setSelected(at);
            scrollTo.current = at;
          }
        }
      } catch (err) {
        if (mine === serial.current) setError(errorText(err));
      } finally {
        if (mine === serial.current) setPhase(null);
      }
    },
    // The target is compared by value.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [targetKey],
  );

  useEffect(() => {
    setSelected(null);
    setFailureId(null);
    setContent(null);
    void load(agent);
  }, [load, agent]);

  useEffect(() => {
    if (scrollTo.current && events.some((e) => e.id === scrollTo.current)) {
      document.getElementById(`trace-row-${scrollTo.current}`)?.scrollIntoView?.({ block: "center" });
      scrollTo.current = null;
    }
  }, [events, lane, prFilter]);

  // The page takes the keyboard from a terminal underneath.
  useEffect(() => {
    if (phase === null) (list.current ?? page.current)?.focus();
  }, [phase]);

  const more = useCallback(async (): Promise<FlightEvent[] | null> => {
    if (!next || phase !== null) return null;
    const mine = serial.current;
    setPhase("more");
    try {
      const v = await flightSession({ ...target, agent, cursor: next });
      if (mine !== serial.current) return null;
      setView(v);
      setEvents((all) => [...all, ...v.page.events]);
      setNext(v.page.nextCursor);
      return v.page.events;
    } catch (err) {
      if (mine === serial.current) setError(errorText(err));
      return null;
    } finally {
      if (mine === serial.current) setPhase(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [next, phase, agent, targetKey]);

  const prByAgent = useMemo(() => prsOf(view?.agents ?? []), [view]);
  const prOf = useCallback(
    (e: FlightEvent) => (isRun(e) ? prOfAttrs(e) : (e.agent.pr ?? prByAgent.get(e.agent.id) ?? null)),
    [prByAgent],
  );
  const lanes = Boolean(target.runDir);
  const shownEvents = useMemo(
    () =>
      events.filter(
        (e) =>
          (!lanes || lane === "both" || (lane === "orchestration") === isRun(e)) && (prFilter === null || prOf(e) === prFilter),
      ),
    [events, lane, lanes, prFilter, prOf],
  );
  const rows = useMemo(() => rowsOf(shownEvents, opened), [shownEvents, opened]);
  const byId = useMemo(() => new Map(events.map((e) => [e.id, e])), [events]);
  const failures = useMemo(() => flatFailures(view?.failures ?? []), [view]);
  const failure = failures.find((f) => f.id === failureId) ?? null;
  const off = view?.status === "off";
  const gone = (view?.notices ?? []).some((n) => n.startsWith("transcript gone"));
  const notices = (view?.notices ?? []).filter((n) => !n.startsWith("transcript gone"));

  const select = (id: string) => {
    setSelected(id);
    document.getElementById(`trace-row-${id}`)?.scrollIntoView?.({ block: "nearest" });
  };

  const jumpToFailure = async (target_: Failure) => {
    if (phase !== null) return;
    setFailureId(target_.id);
    setContent(null);
    // A failure the filters hide is shown again.
    setLane("both");
    setPrFilter(null);
    let known = events.some((e) => e.id === target_.id);
    let cursor = next;
    let pages = 0;
    if (!known && cursor) setPhase("more");
    try {
      // Bring the failing event into the list: fetch later pages until it is there.
      while (!known && cursor && pages < MAX_JUMP_PAGES) {
        const mine = serial.current;
        try {
          const v = await flightSession({ ...target, agent, cursor });
          if (mine !== serial.current) return;
          setEvents((all) => [...all, ...v.page.events]);
          setNext(v.page.nextCursor);
          cursor = v.page.nextCursor;
          known = v.page.events.some((e) => e.id === target_.id);
        } catch (err) {
          setError(errorText(err));
          return;
        }
        pages += 1;
      }
    } finally {
      setPhase((p) => (p === "more" ? null : p));
    }
    if (known) select(target_.id);
  };

  const stepFailure = (by: 1 | -1) => {
    if (failures.length === 0) return;
    const at = failures.findIndex((f) => f.id === failureId);
    const to = at < 0 ? (by === 1 ? 0 : failures.length - 1) : Math.min(failures.length - 1, Math.max(0, at + by));
    void jumpToFailure(failures[to]!);
  };

  const openContent = (event: FlightEvent) => {
    if (off) return;
    if (content?.id === event.id) {
      setContent(null);
      return;
    }
    setCopied(false);
    setContent({ id: event.id, state: "loading" });
    flightContent(target, event.content.ref).then(
      (out) => setContent((now) => (now?.id === event.id ? { id: event.id, state: "ready", out } : now)),
      (err: unknown) =>
        setContent((now) => (now?.id === event.id ? { id: event.id, state: "error", message: errorText(err) } : now)),
    );
  };

  /** The Event list follows the replay cursor: a loaded Event is selected, another one is loaded around. */
  const followCursor = (entries: ReplayTrackOut["entries"], index: number) => {
    const entry = entries[index];
    if (!entry) return;
    setContent(null);
    if (byId.has(entry.id)) {
      scrollTo.current = entry.id;
      select(entry.id);
    } else {
      focusRef.current = entry.id;
      void load(undefined);
    }
    if (entry.kind === "file.change") {
      setFileState({ id: entry.id, state: "loading" });
      flightReplayFile(target, entry.id).then(
        (out) => setFileState((now) => (now?.id === entry.id ? { id: entry.id, state: "ready", out } : now)),
        (err: unknown) =>
          setFileState((now) => (now?.id === entry.id ? { id: entry.id, state: "error", message: errorText(err) } : now)),
      );
    } else {
      setFileState(null);
    }
  };

  const moveCursor = (index: number) => {
    if (!track) return;
    const to = Math.min(Math.max(0, index), Math.max(0, track.entries.length - 1));
    setCursor(to);
    followCursor(track.entries, to);
  };

  const leaveReplay = () => {
    setReplay(false);
    setTrack(null);
    setTrackError(null);
    setFileState(null);
  };

  const enterReplay = async () => {
    // Replay steps through every Event: the agent and lane filters would hide some.
    setAgent(undefined);
    setLane("both");
    setPrFilter(null);
    setFailureId(null);
    setContent(null);
    setReplay(true);
    setTrack(null);
    setTrackError(null);
    try {
      const out = await flightReplayTrack(target);
      setTrack(out);
      const at = Math.max(0, out.entries.findIndex((e) => e.id === selected));
      setCursor(at);
      followCursor(out.entries, at);
    } catch (err) {
      setTrackError(errorText(err));
    }
  };

  const onListKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const at = rows.findIndex((r) => r.id === selected);
    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (at < rows.length - 1) select(rows[at + 1]!.id);
      else if (next) void more().then((added) => added?.[0] && select(added[0].id));
      else if (at < 0 && rows[0]) select(rows[0].id);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (at > 0) select(rows[at - 1]!.id);
      else if (at < 0 && rows[0]) select(rows[0].id);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const row = rows[at];
      if (row?.type === "event") openContent(row.event);
      else if (row?.type === "unknown") setOpened((set) => new Set(set).add(row.id));
    }
  };

  // `]` and `[` jump between failures from anywhere in the page; Escape first closes an open content view.
  const onPageKey = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === "]") stepFailure(1);
    else if (event.key === "[") stepFailure(-1);
    else if (event.key === "Escape" && content) {
      event.stopPropagation();
      setContent(null);
    } else if (event.key === "Escape" && replay) {
      event.stopPropagation();
      leaveReplay();
    }
  };

  const onScroll = () => {
    const el = list.current;
    if (el && next && phase === null && el.scrollTop + el.clientHeight >= el.scrollHeight - 48) void more();
  };

  const before = (failure?.before ?? []).map((id) => byId.get(id)).filter((e): e is FlightEvent => Boolean(e));
  const shown = content ? byId.get(content.id) : undefined;

  return (
    <section className="page trace-panel" role="region" aria-label="Trace" tabIndex={-1} ref={page} onKeyDown={onPageKey}>
      <header className="page-header">
        <h2>Trace</h2>
        <span className="trace-actions">
          {view && !off && <span className="hint">{view.trace}</span>}
          {view?.snapshot && !off && <SnapshotPill info={view.snapshot} />}
          {view && (
            <span className="replay-toggle">
              <button
                type="button"
                className="secondary-button"
                aria-pressed={replay}
                disabled={replayDisabledReason(events.length, off) !== null}
                title={replayDisabledReason(events.length, off) ?? undefined}
                onClick={() => (replay ? leaveReplay() : void enterReplay())}
              >
                Replay
              </button>
            </span>
          )}
          <button
            type="button"
            className="secondary-button"
            disabled={!view || off}
            title={off ? "AgentFlight is off for this account (Settings › Activity)." : undefined}
            onClick={() => setExporting(true)}
          >
            Export
          </button>
          {onSearch && (
            <button type="button" className="secondary-button" onClick={onSearch}>
              Search
            </button>
          )}
          <button type="button" className="secondary-button" disabled={phase !== null} onClick={() => void load(agent)}>
            Refresh
          </button>
          <button type="button" className="secondary-button" onClick={onClose}>
            Close
          </button>
        </span>
      </header>

      {phase === "cold" && (
        <div className="trace-skeleton" aria-busy="true" aria-label="Loading the trace" data-testid="trace-loading">
          {Array.from({ length: PLACEHOLDER_ROWS }, (_, i) => (
            <div key={i} className="trace-skeleton-row" />
          ))}
        </div>
      )}

      {phase !== "cold" && error && (
        <div className="trace-state" role="alert" data-testid="trace-error">
          <p className="error">Could not read the trace: {error}</p>
          <button type="button" className="secondary-button" onClick={() => void load(agent)}>
            Try again
          </button>
        </div>
      )}

      {phase !== "cold" && !error && off && (
        <p className="hint" data-testid="trace-off">
          Reading session logs is turned off for this account. Change it in Settings, Activity, Session Inspector.
        </p>
      )}

      {phase !== "cold" && !error && view && !off && (
        <>
          <p className="hint" data-testid="trace-refresh-note">
            New lines arrive at the end. A line Claude Code writes late with an earlier time appears only after Refresh.
          </p>
          {gone && (
            <p className="hint warn" data-testid="trace-gone">
              A session transcript is gone: Claude Code deletes old logs. What is left is shown.
            </p>
          )}
          {notices.map((n) => (
            <p key={n} className="hint warn" data-testid="trace-notice">
              {n}
            </p>
          ))}
          {events.length === 0 && !gone && <p className="hint">No events were recorded for this run yet.</p>}
          <div className="trace-body">
            <aside className="trace-rail" aria-label="Failures and agents">
              <h3>Failures</h3>
              {failures.length === 0 ? (
                <p className="hint">No failed, blocked or errored tool call.</p>
              ) : (
                <ul className="trace-failures">
                  {view.failures.map((f) => (
                    <li key={f.id}>
                      <button
                        type="button"
                        className="trace-failure"
                        aria-pressed={failureId === f.id}
                        onClick={() => void jumpToFailure(f)}
                      >
                        {clock(f.at)} {f.name ?? flightKindText(f.kind)}{" "}
                        {f.rule && <code className="trace-rule tone-warn">{f.rule}</code>}{" "}
                        <Pill tone="danger" title={`${f.derived.label} · ${f.derived.evidence.length} events`}>
                          {flightDerivedText(f.derived.label)}
                        </Pill>
                      </button>
                      {f.repeats.length > 0 && (
                        <ul>
                          {f.repeats.map((r) => (
                            <li key={r.id}>
                              <button
                                type="button"
                                className="trace-failure"
                                aria-pressed={failureId === r.id}
                                onClick={() => void jumpToFailure(r)}
                              >
                                {clock(r.at)} Repeat of {clock(f.at)}{" "}
                                <Pill tone="muted" title={r.repeatOf?.label}>
                                  heuristic
                                </Pill>
                              </button>
                            </li>
                          ))}
                        </ul>
                      )}
                    </li>
                  ))}
                </ul>
              )}
              <h3>Agents</h3>
              <AgentTree
                nodes={view.agents}
                agent={agent}
                onPick={(id) => {
                  if (replay) leaveReplay();
                  setAgent(id);
                }}
              />
              {agent && (
                <button type="button" className="secondary-button" onClick={() => setAgent(undefined)}>
                  All agents
                </button>
              )}
            </aside>

            <div className="trace-main">
              {failure && (
                <div className="trace-context" data-testid="trace-context">
                  <h3>Before this failure</h3>
                  {before.length === 0 ? (
                    <p className="hint">Nothing earlier from this agent is loaded.</p>
                  ) : (
                    <ol>
                      {before.map((e) => (
                        <li key={e.id}>
                          {clock(e.at)} {flightKindText(e.kind)}
                          {e.name ? `: ${e.name}` : ""}
                        </li>
                      ))}
                    </ol>
                  )}
                </div>
              )}
              {replay && (
                <div className="trace-replay" data-testid="trace-replay">
                  {trackError && <p className="error">Could not read the replay track: {trackError}</p>}
                  {!trackError && !track && (
                    <div className="replay-skeleton" aria-busy="true" aria-label="Loading the replay track" data-testid="replay-loading" />
                  )}
                  {track && track.status === "ok" && (
                    <ReplayTrack entries={track.entries} cursor={cursor} onCursor={moveCursor} truncated={track.truncated} />
                  )}
                </div>
              )}
              {lanes && !replay && (
                <div
                  className="trace-lanes"
                  role="radiogroup"
                  aria-label="Lanes"
                  onKeyDown={(event) => {
                    const at = LANES.findIndex((l) => l.id === lane);
                    const by = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
                    if (by === 0) return;
                    event.preventDefault();
                    const to = LANES[(at + by + LANES.length) % LANES.length]!;
                    setLane(to.id);
                    requestAnimationFrame(() => document.getElementById(`trace-lane-${to.id}`)?.focus());
                  }}
                >
                  {LANES.map((l) => (
                    <button
                      key={l.id}
                      id={`trace-lane-${l.id}`}
                      type="button"
                      role="radio"
                      aria-checked={lane === l.id}
                      tabIndex={lane === l.id ? 0 : -1}
                      className="trace-lane-choice"
                      onClick={() => setLane(l.id)}
                    >
                      {l.label}
                    </button>
                  ))}
                </div>
              )}
              {prFilter && (
                <p className="hint" data-testid="trace-pr-filter">
                  Showing {prFilter} only.{" "}
                  <button type="button" className="secondary-button" onClick={() => setPrFilter(null)}>
                    Show all PRs
                  </button>
                </p>
              )}
              <div className={`trace-split${replay && fileState ? " trace-split-file" : ""}`}>
              <div className="trace-list" ref={list} onScroll={onScroll} role="presentation">
                <div
                  className="trace-events"
                  role="listbox"
                  aria-label="Events"
                  tabIndex={0}
                  aria-activedescendant={selected ? `trace-row-${selected}` : undefined}
                  onKeyDown={onListKey}
                  data-testid="trace-events"
                >
                  {rows.map((row) =>
                    row.type === "unknown" ? (
                      <div
                        key={row.id}
                        id={`trace-row-${row.id}`}
                        role="option"
                        aria-selected={selected === row.id}
                        className="trace-row trace-row-unknown"
                        onClick={() => setOpened((set) => new Set(set).add(row.id))}
                      >
                        {row.count} unknown lines
                      </div>
                    ) : (
                      <div
                        key={row.id}
                        id={`trace-row-${row.id}`}
                        role="option"
                        aria-selected={selected === row.id}
                        className={`trace-row${isRun(row.event) ? " trace-row-run" : ""}${failureId === row.id ? " trace-row-failure" : ""}`}
                        onClick={() => {
                          setSelected(row.id);
                          openContent(row.event);
                        }}
                      >
                        <span className="trace-time">{clock(row.event.at)}</span>{" "}
                        {isRun(row.event) ? (
                          <>
                            <span className="trace-lane-chip">run</span>{" "}
                            <span>{orchestrationText(row.event)}</span>
                            {orchestrationStatusText(row.event) && (
                              <>
                                {" "}
                                <Pill tone={orchestrationTone(row.event.kind, row.event.status)}>
                                  {orchestrationStatusText(row.event)}
                                </Pill>
                              </>
                            )}
                          </>
                        ) : (
                          <>
                            <span className="trace-agent-name">{agentName(row.event.agent)}</span>{" "}
                            <span>
                              {flightKindText(row.event.kind)}
                              {row.event.name ? `: ${row.event.name}` : ""}
                            </span>
                            {row.event.status !== null && (
                              <>
                                {" "}
                                <Pill tone={flightStatusTone(row.event.status)}>{flightStatusText(row.event.status)}</Pill>
                              </>
                            )}
                          </>
                        )}
                        {ruleOf(row.event) && (
                          <>
                            {" "}
                            <code className="trace-rule tone-warn">{ruleOf(row.event)}</code>
                          </>
                        )}
                        {prOf(row.event) && (
                          <>
                            {" "}
                            <button
                              type="button"
                              className="pill tone-muted trace-pr-button"
                              aria-label={`Show only ${prOf(row.event)}`}
                              aria-pressed={prFilter === prOf(row.event)}
                              onClick={(event) => {
                                event.stopPropagation();
                                setPrFilter((now) => (now === prOf(row.event) ? null : prOf(row.event)));
                              }}
                            >
                              {prOf(row.event)}
                            </button>
                          </>
                        )}
                      </div>
                    ),
                  )}
                </div>
                {phase === "more" && (
                  <p className="hint" data-testid="trace-more">
                    Loading more
                  </p>
                )}
                {next && phase === null && (
                  <button type="button" className="secondary-button" onClick={() => void more()}>
                    Load more
                  </button>
                )}
              </div>

              {replay && fileState && <FilePane file={fileState} />}
              </div>

              {content && (
                <div className="trace-content" data-testid="trace-content">
                  {content.state === "loading" && <p className="hint">Reading the line…</p>}
                  {content.state === "error" && (
                    <p className="error">
                      {content.message === CONTENT_NOT_KEPT
                        ? `Content not kept (policy ${view?.snapshot?.policy ?? view?.policy ?? "metadata"})`
                        : content.message}
                    </p>
                  )}
                  {content.state === "ready" && (
                    <>
                      {shown && <EventFields event={shown} phase={view?.phases?.[shown.id]} />}
                      <pre className={`trace-pre${shown && isRun(shown) ? " trace-pre-run" : ""}`}>{content.out.text}</pre>
                      {replay && shown && target.runDir && shown.kind === "check.end" && prOf(shown) && (
                        <RerunCheck runDir={target.runDir} pr={prOf(shown)!} appCheck={shown.attrs.source === "app"} />
                      )}
                      <p className="hint">
                        {content.out.redactions} redacted
                        {shown ? ` · source ${shown.content.ref.file.split(/[\\/]/).pop()} @ ${shown.content.ref.offset}+${shown.content.ref.len}` : ""}
                      </p>
                      <button
                        type="button"
                        className="secondary-button"
                        onClick={() => {
                          void navigator.clipboard
                            .writeText(content.out.text)
                            .then(() => setCopied(true), () => setCopied(false));
                        }}
                      >
                        {copied ? "Copied" : "Copy"}
                      </button>
                    </>
                  )}
                </div>
              )}
            </div>
          </div>
        </>
      )}
      {exporting && view && !off && (
        <ExportDialog target={target} capturePolicy={view.policy} trace={view.trace} onClose={() => setExporting(false)} />
      )}
    </section>
  );
}
