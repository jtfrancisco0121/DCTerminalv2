import { describe, expect, it } from "vitest";
import app from "../../fixtures/fake/app.json";
import clickup from "../../fixtures/fake/clickup.json";
import run from "../../fixtures/fake/run.json";
import runs from "../../fixtures/fake/runs.json";
import settings from "../../fixtures/fake/settings.json";
import shapes from "../../fixtures/fake/shapes.json";

// fixtures/fake/shapes.json is written by the cargo test in src-tauri/src/fake_shapes.rs from the real Rust return
// types. Each fixture must keep to its command's shape: no missing key (unless the shape marks it optional), no
// extra key, and the same JSON type wherever the fixture value is not null.
export type Shape = {
  type: string;
  nullable?: boolean;
  optional?: boolean;
  fields?: Record<string, Shape>;
  items?: Shape | null;
  /** An object whose keys are data: every value has this shape. */
  map?: Shape | null;
  /** An object of free-form JSON: any keys and values. */
  free?: boolean;
};

const typeOf = (value: unknown): string =>
  value === null ? "null" : Array.isArray(value) ? "array" : typeof value;

/** Every way `value` differs from `shape`, as `path: problem` lines (empty: it fits). */
export function mismatches(value: unknown, shape: Shape, path = "$"): string[] {
  if (value === null) return [];
  const got = typeOf(value);
  if (shape.type === "null") return []; // the sample held a None: the inner type is not known
  if (got !== shape.type) return [`${path}: expected ${shape.type}, got ${got}`];
  const out: string[] = [];
  if (shape.type === "object" && shape.free) return [];
  if (shape.type === "object" && "map" in shape) {
    const inner = shape.map;
    if (inner) {
      for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
        out.push(...mismatches(item, inner, `${path}.${key}`));
      }
    }
    return out;
  }
  if (shape.type === "object") {
    const fields = shape.fields ?? {};
    const have = value as Record<string, unknown>;
    for (const [key, field] of Object.entries(fields)) {
      if (!(key in have) && !field.optional) out.push(`${path}.${key}: missing`);
    }
    for (const key of Object.keys(have)) {
      if (!(key in fields)) out.push(`${path}.${key}: not in the Rust type`);
    }
    for (const [key, field] of Object.entries(fields)) {
      if (key in have) out.push(...mismatches(have[key], field, `${path}.${key}`));
    }
  } else if (shape.type === "array" && shape.items) {
    (value as unknown[]).forEach((item, index) => out.push(...mismatches(item, shape.items!, `${path}[${index}]`)));
  }
  return out;
}

const fixtures: Record<string, unknown> = { ...app, ...clickup, ...run, ...runs, ...settings };
const shapeOf = shapes as unknown as Record<string, Shape>;

describe("fixtures keep to the shapes of the Rust types", () => {
  it("has a shape for every fixture and a fixture for every shape", () => {
    expect(Object.keys(shapeOf).sort()).toEqual(Object.keys(fixtures).sort());
  });

  for (const command of Object.keys(fixtures)) {
    it(`${command} fits its shape`, () => {
      expect(mismatches(fixtures[command], shapeOf[command]!)).toEqual([]);
    });
  }

  it("fails a fixture with a missing key, an extra key or a wrong type", () => {
    const shape: Shape = {
      type: "object",
      fields: { cap: { type: "number" }, note: { type: "string", optional: true }, list: { type: "array", items: { type: "string" } } },
    };
    expect(mismatches({ cap: 3, list: ["a"] }, shape)).toEqual([]);
    expect(mismatches({ list: [] }, shape)).toEqual(["$.cap: missing"]);
    expect(mismatches({ cap: 3, list: [], extra: 1 }, shape)).toEqual(["$.extra: not in the Rust type"]);
    expect(mismatches({ cap: "3", list: [] }, shape)).toEqual(["$.cap: expected number, got string"]);
    expect(mismatches({ cap: 3, list: [1] }, shape)).toEqual(["$.list[0]: expected string, got number"]);
    expect(mismatches({ cap: null, list: [] }, shape)).toEqual([]);
    const keyed: Shape = { type: "object", map: { type: "boolean" } };
    expect(mismatches({ any: true, other: false }, keyed)).toEqual([]);
    expect(mismatches({ any: "yes" }, keyed)).toEqual(["$.any: expected boolean, got string"]);
    expect(mismatches({ a: 1, b: [2] }, { type: "object", free: true })).toEqual([]);
  });

  it("fails the old Runs fixture shape (queue and running, no holders)", () => {
    const old = { queue: [], running: [], epics: [] };
    expect(mismatches(old, shapeOf["dispatch_overview"]!).length).toBeGreaterThan(0);
  });
});
