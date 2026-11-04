import { describe, expect, it } from "vitest";
import sample from "../../src-tauri/src/fixtures/flight/events-sample.json";
import runSample from "../../src-tauri/src/fixtures/flight/run-events-sample.json";
import { ATTR, KIND, ORCHESTRATION, PROVIDER_DCV2, STATUS } from "./types";
import type { FlightEvent } from "./types";

// The sample is the Rust serialization (checked by cargo test); this makes
// the TS mirror fail to compile when the two shapes drift.
const events = sample satisfies FlightEvent[];

const KEYS = [
  "agent",
  "at",
  "attrs",
  "content",
  "derived",
  "id",
  "kind",
  "name",
  "provider",
  "schema",
  "session",
  "span",
  "status",
  "trace",
  "usage",
];

describe("flight types", () => {
  it("the Rust sample type-checks as FlightEvent[] and has the schema keys", () => {
    expect(events.length).toBeGreaterThan(0);
    for (const event of events) {
      expect(Object.keys(event).sort()).toEqual(KEYS);
      expect(event.schema).toBe(1);
      expect(Object.keys(event.content).sort()).toEqual(["policy", "redactions", "ref"]);
      expect(Object.keys(event.content.ref).sort()).toEqual(["file", "len", "offset", "sha256"]);
      expect(Object.keys(event.agent).sort()).toEqual(["id", "kind", "parent", "pr", "role", "type"]);
    }
  });

  it("every new orchestration kind has a serialized Event whose kind, status and attr keys are TS constants", () => {
    const orchestration = runSample satisfies FlightEvent[];
    const kinds = new Set<string>(Object.values(KIND));
    const statuses = new Set<string>([...Object.values(STATUS), "error", "cancelled"]);
    const attrKeys = new Set<string>(Object.values(ATTR));
    const seen = new Set<string>();
    for (const event of orchestration) {
      expect(Object.keys(event).sort()).toEqual(KEYS);
      expect(kinds.has(event.kind), event.kind).toBe(true);
      if (event.status !== null) expect(statuses.has(event.status), event.status).toBe(true);
      for (const key of Object.keys(event.attrs)) expect(attrKeys.has(key), key).toBe(true);
      expect(event.agent.id).toBe(ORCHESTRATION);
      expect(event.provider.name).toBe(PROVIDER_DCV2);
      seen.add(event.kind);
    }
    const newKinds = Object.values(KIND).filter(
      (k) => !["session.start", "turn.start", "turn.end", "llm.call", "tool.start", "tool.end", "agent.spawn", "file.change", "error", "x.claude.unknown"].includes(k),
    );
    for (const k of newKinds) expect(seen.has(k), `${k} has no sample`).toBe(true);
    expect([...seen].filter((k) => k.startsWith("x.")).every((k) => k.startsWith("x.dcv2."))).toBe(true);
  });
});
