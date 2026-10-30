import { describe, expect, it } from "vitest";
import { splitGroups } from "./groups";

describe("splitGroups", () => {
  it("keeps every ticket in one run by default", () => {
    expect(splitGroups(["a", "b", "c"], { perTicket: false, letters: {} })).toEqual([["a", "b", "c"]]);
    expect(splitGroups([], { perTicket: false, letters: {} })).toEqual([]);
  });

  it("gives each ungrouped ticket its own run", () => {
    expect(splitGroups(["a", "b", "c"], { perTicket: true, letters: {} })).toEqual([["a"], ["b"], ["c"]]);
  });

  it("puts tickets with the same letter in one run and the rest in their own, in first-seen order", () => {
    expect(splitGroups(["a", "b", "c", "d"], { perTicket: true, letters: { a: "x", c: "X", d: " " } })).toEqual([["a", "c"], ["b"], ["d"]]);
  });

  it("uses only the first character of a letter", () => {
    expect(splitGroups(["a", "b"], { perTicket: true, letters: { a: "gh", b: "G" } })).toEqual([["a", "b"]]);
  });
});
