import { useEffect, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { flightSearch } from "../flight/api";
import type { RunHit, SearchFilter, SearchOut } from "../flight/types";
import { errorText } from "./InlineNotice";
import { Meter } from "./Meter";
import { Pill } from "./Pill";

/** What the form holds; kept in the App while it runs, so Escape from a Trace returns to the same results. */
export interface SearchForm {
  kind: string;
  nameContains: string;
  status: string;
  stage: string;
  prState: string;
  prRole: string;
  inReview: boolean;
  repo: string;
  from: string;
  to: string;
  text: string;
}

export interface SearchState {
  form: SearchForm;
  /** The filter of the last search that returned, and its answer. */
  filter: SearchFilter | null;
  out: SearchOut | null;
}

export const EMPTY_FORM: SearchForm = {
  kind: "",
  nameContains: "",
  status: "",
  stage: "",
  prState: "",
  prRole: "",
  inReview: false,
  repo: "",
  from: "",
  to: "",
  text: "",
};

const STATUSES: [string, string][] = [
  ["", "Any status"],
  ["failed", "Failed (fail or error)"],
  ["fail", "fail"],
  ["error", "error"],
  ["pass", "pass"],
  ["ok", "ok"],
  ["blocked", "blocked"],
  ["skip", "skip"],
];
const STAGES = ["planning", "plan-approval", "building", "ship-approval", "shipping", "done", "failed"];
const PR_STATES = ["pending", "building", "drafted", "reviewing", "fixing", "reviewed", "needs-attention", "opened", "pushed", "local-only"];
const ROLES = ["implementer", "pr-reviewer", "planner", "plan-reviewer", "risk-reviewer"];

/** The structured filter of a form: only the fields that are set. Values only, never SQL. */
export function filterOf(form: SearchForm): SearchFilter {
  const filter: SearchFilter = {};
  const put = (key: "kind" | "nameContains" | "stage" | "prState" | "prRole" | "repo" | "from" | "to" | "text") => {
    const value = form[key].trim();
    if (value) filter[key] = value;
  };
  (["kind", "nameContains", "stage", "prState", "prRole", "repo", "from", "to", "text"] as const).forEach(put);
  if (form.status) filter.status = [form.status];
  if (form.inReview) filter.inReview = true;
  return filter;
}

const LABELS: Record<string, string> = {
  kind: "Kind",
  nameContains: "Name",
  status: "Status",
  stage: "Stage",
  prState: "PR state",
  prRole: "PR role",
  repo: "Repo",
  from: "From",
  to: "To",
  text: "Text",
};

/** The active filters as short pills: "Kind: check.end". */
export function pillsOf(filter: SearchFilter): { key: string; text: string }[] {
  const out: { key: string; text: string }[] = [];
  for (const [key, label] of Object.entries(LABELS)) {
    const value = (filter as Record<string, unknown>)[key];
    if (Array.isArray(value) && value.length) out.push({ key, text: `${label}: ${value.join(", ")}` });
    else if (typeof value === "string" && value) out.push({ key, text: `${label}: ${value}` });
  }
  if (filter.inReview) out.push({ key: "inReview", text: "During review" });
  return out;
}

type Props = {
  /** The last form and results, kept by the App. */
  initial: SearchState | null;
  onChange: (state: SearchState) => void;
  /** Open this run's Trace on its first match. */
  onOpen: (hit: RunHit) => void;
  onClose: () => void;
};

/**
 * Trace search: structured filters over the local index (no query text), results as runs with their match
 * counts, and a result opens the run's Trace on the first matching event. Nothing searches while you type.
 */
export function TraceSearch({ initial, onChange, onOpen, onClose }: Props) {
  const [form, setForm] = useState<SearchForm>(initial?.form ?? EMPTY_FORM);
  const [filter, setFilter] = useState<SearchFilter | null>(initial?.filter ?? null);
  const [out, setOut] = useState<SearchOut | null>(initial?.out ?? null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const page = useRef<HTMLElement>(null);
  const busyRef = useRef(false);
  const latest = useRef({ form, filter, out });
  latest.current = { form, filter, out };

  // The page takes the keyboard from a terminal underneath.
  useEffect(() => page.current?.focus(), []);

  const set = <K extends keyof SearchForm>(key: K, value: SearchForm[K]) => setForm((f) => ({ ...f, [key]: value }));

  /** One search at a time: a second submit while one runs is ignored. */
  const run = async (next: SearchForm) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    const asked = filterOf(next);
    try {
      const answer = await flightSearch(asked);
      setOut(answer);
      setFilter(asked);
      setError(null);
      setActive(0);
      onChange({ form: next, filter: asked, out: answer });
    } catch (err) {
      // The previous results stay.
      setError(errorText(err));
      onChange({ form: next, filter: latest.current.filter, out: latest.current.out });
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void run(form);
  };

  const clear = () => {
    setForm(EMPTY_FORM);
    setFilter(null);
    setOut(null);
    setError(null);
    onChange({ form: EMPTY_FORM, filter: null, out: null });
  };

  const runs = out?.runs ?? [];
  const behind = out !== null && out.progress.current < out.progress.total;
  const onListKey = (event: KeyboardEvent<HTMLUListElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((i) => Math.min(runs.length - 1, i + 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((i) => Math.max(0, i - 1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const hit = runs[active];
      if (hit) onOpen(hit);
    }
  };

  const select = (name: keyof SearchForm, label: string, options: [string, string][]) => (
    <label className="trace-search-field">
      <span>{label}</span>
      <select value={String(form[name])} onChange={(e) => set(name, e.target.value as never)}>
        {options.map(([value, text]) => (
          <option key={value} value={value}>
            {text}
          </option>
        ))}
      </select>
    </label>
  );
  const input = (name: "kind" | "nameContains" | "repo" | "text", label: string, placeholder: string) => (
    <label className="trace-search-field">
      <span>{label}</span>
      <input type="text" value={form[name]} placeholder={placeholder} onChange={(e) => set(name, e.target.value)} />
    </label>
  );
  const anyOf = (values: string[], any: string): [string, string][] => [["", any], ...values.map((v): [string, string] => [v, v])];

  return (
    <section className="page trace-search" role="region" aria-label="Trace search" tabIndex={-1} ref={page}>
      <header className="page-header">
        <h2>Search traces</h2>
        <button type="button" className="secondary-button" onClick={onClose}>
          Close
        </button>
      </header>

      <form className="trace-search-form" onSubmit={submit} aria-label="Trace filters">
        {input("kind", "Kind", "check.end")}
        {input("nameContains", "Name contains", "cargo test")}
        {select("status", "Status", STATUSES)}
        {select("stage", "Stage", anyOf(STAGES, "Any stage"))}
        {select("prState", "PR state", anyOf(PR_STATES, "Any PR state"))}
        {select("prRole", "PR role", anyOf(ROLES, "Any role"))}
        {input("repo", "Repo", "name")}
        <label className="trace-search-field">
          <span>From</span>
          <input type="date" value={form.from} onChange={(e) => set("from", e.target.value)} />
        </label>
        <label className="trace-search-field">
          <span>To</span>
          <input type="date" value={form.to} onChange={(e) => set("to", e.target.value)} />
        </label>
        {input("text", "Names and paths", "src/App.tsx")}
        <div className="trace-search-actions">
          <button
            type="button"
            className="secondary-button"
            aria-pressed={form.inReview}
            onClick={() => set("inReview", !form.inReview)}
          >
            During review
          </button>
          <button type="submit" className="primary-button" disabled={busy}>
            {busy ? "Searching…" : "Search"}
          </button>
          <button type="button" className="secondary-button" onClick={clear}>
            Clear
          </button>
        </div>
      </form>

      {filter && pillsOf(filter).length > 0 && (
        <ul className="trace-search-pills" aria-label="Active filters">
          {pillsOf(filter).map((p) => (
            <li key={p.key}>
              <Pill tone="muted">{p.text}</Pill>
            </li>
          ))}
        </ul>
      )}

      {error && (
        <p className="error" role="alert" data-testid="search-error">
          Could not search: {error}
        </p>
      )}

      {behind && out && (
        <p className="hint" data-testid="search-progress">
          Indexing {out.progress.current} of {out.progress.total} runs{" "}
          <Meter
            value={out.progress.total > 0 ? (out.progress.current / out.progress.total) * 100 : null}
            tone="running"
            label={`Indexing ${out.progress.current} of ${out.progress.total} runs`}
          />
        </p>
      )}

      {out === null && !error && <p className="hint">Set filters and press Search. Nothing searches while you type.</p>}

      {out !== null && runs.length === 0 && (
        <p className="hint" data-testid="search-empty">
          {out.progress.current === 0 ? "No runs indexed yet. Indexing runs in the background." : "No runs match."}
        </p>
      )}

      {out !== null && out.excludedOff > 0 && (
        <p className="hint" data-testid="search-excluded">
          {out.excludedOff} {out.excludedOff === 1 ? "run" : "runs"} excluded: AgentFlight is off for their account
        </p>
      )}

      {runs.length > 0 && (
        <ul
          className="trace-search-results"
          role="listbox"
          aria-label="Matching runs"
          tabIndex={0}
          aria-activedescendant={`search-run-${runs[Math.min(active, runs.length - 1)]!.runId}`}
          onKeyDown={onListKey}
        >
          {runs.map((hit, i) => (
            <li
              key={hit.runId}
              id={`search-run-${hit.runId}`}
              role="option"
              aria-selected={i === active}
              className="trace-search-result"
              onClick={() => onOpen(hit)}
            >
              <strong>{hit.title || hit.runId}</strong>{" "}
              <span className="hint">
                {hit.runId}
                {hit.outcome ? ` · ${hit.outcome}` : ""}
              </span>{" "}
              <Pill tone="muted">
                {hit.matches} {hit.matches === 1 ? "match" : "matches"}
              </Pill>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
