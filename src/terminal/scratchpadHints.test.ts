import { describe, expect, it } from "vitest";
import { draftHints, sendBlockReason } from "./scratchpadHints";

describe("scratchpad hints", () => {
  it("says nothing about an empty draft", () => {
    expect(draftHints("  ")).toEqual([]);
  });

  it("hints at missing acceptance criteria, files and length, and never more", () => {
    const hints = draftHints("make it better please");
    expect(hints.some((h) => h.startsWith("No acceptance criteria"))).toBe(true);
    expect(hints.some((h) => h.includes("names no file or area"))).toBe(true);
    expect(draftHints("x".repeat(4001)).some((h) => h.startsWith("Very long"))).toBe(true);
  });

  it("is quiet for a draft with criteria and a file", () => {
    expect(draftHints("In src/App.tsx the tab title must not overflow; done when the title is cut with an ellipsis.")).toEqual([]);
  });
});

describe("Send rules", () => {
  it("allows one line anywhere", () => {
    expect(sendBlockReason("fix the bug", false)).toBeNull();
  });

  it("blocks any newline, a trailing one included, without bracketed paste", () => {
    expect(sendBlockReason("a\nb", false)).not.toBeNull();
    expect(sendBlockReason("one line\n", false)).not.toBeNull();
    expect(sendBlockReason("a\nb", true)).toBeNull();
    expect(sendBlockReason("one line\n", true)).toBeNull();
  });

  it("blocks an empty draft", () => {
    expect(sendBlockReason("   ", true)).not.toBeNull();
  });
});
