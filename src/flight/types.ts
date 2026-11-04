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
}
