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
  /** Set when a session whose transcript is gone was read from the run's snapshot. */
  snapshot?: SnapshotInfo | null;
}

/** The run snapshot a gone session was read from (snapshot.rs `Manifest`, host.rs `session_view`). */
export interface SnapshotInfo {
  createdAt: string;
  /** The capture policy the snapshot was made under. */
  policy: CapturePolicy;
  /** The snapshot holds redacted content (`content.jsonl`). */
  contentKept: boolean;
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

/** One side of a comparison (host.rs `CompareSide`). */
export interface CompareSide {
  runId: string;
  runDir: string;
  title: string;
  outcome: string;
  /** The run is in the trace index. */
  indexed: boolean;
  /** AgentFlight is off for the run's account: nothing of it is read. */
  off: boolean;
}

/** The groups of a Compare row, in the fixed order of the page (compare.rs `GROUPS`). */
export const COMPARE_GROUPS = [
  "stages",
  "tools",
  "failures",
  "retries",
  "repeats",
  "files",
  "checks",
  "tokens",
  "windowShare",
] as const;
export type CompareGroup = (typeof COMPARE_GROUPS)[number];

/** One Compare row: a number, a text or null (not recorded) on each side. */
export interface CompareRow {
  group: CompareGroup | string;
  key: string;
  a: number | string | null;
  b: number | string | null;
  differs: boolean;
  /** The first Event of the row in run A / run B (none for record numbers). */
  aEvent?: string | null;
  bEvent?: string | null;
}

/** What `flight_compare` returns. `rows` is empty unless both sides are indexed and readable. */
export interface CompareOut {
  a: CompareSide;
  b: CompareSide;
  rows: CompareRow[];
  /** The index (into `rows`) of the first row that differs. */
  divergence: number | null;
  progress: { current: number; total: number };
}


/** min / median / max of one measure over the finished runs that have it (host.rs `Dist`). */
export interface Dist {
  min: number | null;
  median: number | null;
  max: number | null;
  /** How many runs the figures are over. */
  of: number;
}

/** A branch a finished eval run left behind (janitor.rs `KeptBranch`). */
export interface KeptBranch {
  prId: string;
  branch: string;
  repoPath: string;
  /** Commits past the run's base commit; null when they could not be counted. */
  unmergedCommits: number | null;
}

/** One run of an eval group (host.rs `EvalRunResult`). A measure that is not known is null, never 0. */
export interface EvalRunResult {
  index: number;
  label: string;
  runId: string | null;
  runDir: string | null;
  /** `finished` (eval-done), `failed`, `running` or `queued`. */
  status: "finished" | "failed" | "running" | "queued" | string;
  completed: boolean | null;
  checkPassRate: number | null;
  /** `checked`, `checking` or `not checked`. */
  checkState: "checked" | "checking" | "not checked" | string;
  retries: number | null;
  toolFailures: number | null;
  tokens: number | null;
  activeMinutes: number | null;
  keptBranches: KeptBranch[];
}

export interface EvalDistributions {
  completed: Dist;
  checkPassRate: Dist;
  retries: Dist;
  toolFailures: Dist;
  tokens: Dist;
  activeMinutes: Dist;
}

/** What `flight_eval_results` returns. */
export interface EvalResults {
  group: string;
  source: string;
  n: number;
  runs: EvalRunResult[];
  distributions: EvalDistributions;
  finished: number;
  running: number;
  failed: number;
}

/** What `flight_export_preview` returns (host.rs `ExportPreview`): the payload's counts, never its text. */
export type ExportPolicy = "metadata" | "full";
export type ExportEncoding = "json" | "protobuf";

export interface ExportField {
  field: string;
  /** How many spans or events carry the attribute. */
  count: number;
  leaves: number;
}

export interface ExportPreview {
  /** The policy actually used (`full` is clamped to `metadata` without its conditions). */
  policy: ExportPolicy;
  encoding: ExportEncoding;
  endpoint: string;
  spans: number;
  events: number;
  bytes: number;
  digest: string;
  fields: ExportField[];
  warnings: string[];
  /** The endpoint is on this machine. */
  loopback: boolean;
  capturePolicy: CapturePolicy;
}

export interface ExportSent {
  endpoint: string;
  spans: number;
  status: number;
}
