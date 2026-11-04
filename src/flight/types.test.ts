import { describe, expect, it } from "vitest";
import sample from "../../src-tauri/src/fixtures/flight/events-sample.json";
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
});
