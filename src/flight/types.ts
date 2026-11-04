// Mirrors src-tauri/src/flight/schema.rs (schema 1, camelCase). Events carry
// refs to their source, never content text.

export type CapturePolicy = "full" | "redacted" | "metadata" | "off";

export interface SourceRef {
  file: string;
  offset: number;
  len: number;
  sha256: string;
}

export interface Content {
  ref: SourceRef;
  /** A `CapturePolicy` (JSON imports widen literals, so it stays a string here). */
  policy: string;
  redactions: number;
}

export interface AgentRef {
  id: string;
  parent: string | null;
  /** `main` or `subagent`. */
  kind: string;
  type: string | null;
  role: string | null;
  pr: string | null;
}

export interface Provider {
  name: string;
  version: string | null;
  native: string;
}

export interface Usage {
  input: number;
  output: number;
  cacheWrite5m: number;
  cacheWrite1h: number;
  cacheRead: number;
  model: string | null;
  usd: number | null;
}

/** Event kinds (schema.rs `kind`): the core ones, then the run folder's orchestration kinds. */
export const KIND = {
  SESSION_START: "session.start",
  TURN_START: "turn.start",
  TURN_END: "turn.end",
  LLM_CALL: "llm.call",
  TOOL_START: "tool.start",
  TOOL_END: "tool.end",
  TOOL_BLOCKED: "tool.blocked",
  AGENT_SPAWN: "agent.spawn",
  FILE_CHANGE: "file.change",
  ERROR: "error",
  UNKNOWN: "x.claude.unknown",
  STAGE_CHANGE: "stage.change",
  APPROVAL_WAIT: "approval.wait",
  APPROVAL_DECISION: "approval.decision",
  CHECK_END: "check.end",
  REVIEW_ROUND: "review.round",
  CI_STATE: "ci.state",
  RETRY: "retry",
  ATTENTION: "attention",
  PR_STATE: "x.dcv2.pr-state",
  LOG: "x.dcv2.log",
  REQUEST: "x.dcv2.request",
} as const;

/** `status` values of the orchestration events (schema.rs `status`), beside `ok`, `error`, `blocked`. */
export const STATUS = {
  PASS: "pass",
  FAIL: "fail",
  SKIP: "skip",
  OK: "ok",
  DECLINED: "declined",
  BLOCKED: "blocked",
} as const;

/** Keys of `FlightEvent.attrs` the orchestration events use (schema.rs `attr`). */
export const ATTR = {
  SOURCE: "source",
  PR: "pr",
  ROUND: "round",
  VERDICT: "verdict",
  HEAD: "head",
  EXIT_CODE: "exitCode",
  CI: "ci",
  PR_STATE: "prState",
  RULE: "rule",
  TOOL: "tool",
  FROM: "from",
  TO: "to",
  REQUEST_KIND: "requestKind",
  RESULT: "result",
  AGENT: "agent",
} as const;

/** The lane of the run folder's own events, and the provider that writes them. */
export const ORCHESTRATION = "orchestration";
export const PROVIDER_DCV2 = "dcv2";

export interface FlightEvent {
  /** Always 1. */
  schema: number;
  id: string;
  trace: string;
  session: string;
  agent: AgentRef;
  provider: Provider;
  kind: string;
  at: string;
  span: string | null;
  status: string | null;
  name: string | null;
  attrs: Record<string, unknown>;
  usage: Usage | null;
  content: Content;
  derived: string | null;
}

export interface Derived {
  label: string;
  evidence: string[];
}

export interface FlightSpan {
  span: string;
  agent: string;
  name: string | null;
  start: string;
  end: string | null;
  startAt: string;
  endAt: string | null;
  status: string | null;
  durationMs: number | null;
  derived: Derived;
}

export interface AgentNode {
  id: string;
  session: string;
  /** `main` or `subagent`. */
  kind: string;
  type: string | null;
  role: string | null;
  pr: string | null;
  prDerived: Derived | null;
  description: string | null;
  spawnSpan: string | null;
  files: string[];
  children: AgentNode[];
  derived: Derived;
}

export interface Failure {
  id: string;
  agent: string;
  at: string;
  name: string | null;
  kind: string;
  derived: Derived;
  before: string[];
  repeatOf: Derived | null;
  repeats: Failure[];
  /** The guard rule id, when the guard's own log names this block. */
  rule?: string | null;
}

/** Where in the run an event happened (derive.rs `phases`). `label` is `exact:log-line` or `heuristic:status-now`. */
export interface Phase {
  stage: string | null;
  prState: string | null;
  label: string | null;
}

/** One page of events and the cursor of the next, or null on the last. */
export interface Page {
  events: FlightEvent[];
  nextCursor: string | null;
}

/** What `flight_session` returns. `off`: the account's policy lets nothing be read. */
export interface SessionView {
  status: "ok" | "off" | string;
  trace: string;
  policy: CapturePolicy;
  agents: AgentNode[];
  failures: Failure[];
  page: Page;
  notices: string[];
  /** Phase of each event of the page, by event id. */
  phases?: Record<string, Phase>;
}

/** What `flight_content` returns: redacted text and how many lines were redacted. */
export interface ContentOut {
  text: string;
  redactions: number;
  policy: CapturePolicy;
}

/** Which transcripts: a run's recorded sessions, or one session of one account folder. */
export interface TraceTarget {
  runDir?: string;
  configDir?: string;
  sessionId?: string;
}

/** The filter of `flight_search` (index.rs `SearchFilter`). `failed` in `status` means `fail` or `error`. */
export interface SearchFilter {
  kind?: string;
  nameContains?: string;
  status?: string[];
  stage?: string;
  prState?: string;
  prRole?: string;
  /** PR state `reviewing` or agent role `pr-reviewer`. */
  inReview?: boolean;
  repo?: string;
  from?: string;
  to?: string;
  text?: string;
}

/** One run with matches: the count, and the first matching Event the Trace opens on. */
export interface RunHit {
  runId: string;
  runDir: string;
  title: string;
  outcome: string;
  matches: number;
  firstEventId: string;
}

export interface SearchOut {
  runs: RunHit[];
  /** Runs left out because their account's capture policy is off. */
  excludedOff: number;
  progress: { current: number; total: number };
}
