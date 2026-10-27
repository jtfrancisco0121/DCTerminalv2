import { describe, expect, it } from "vitest";
import { dropPointToCss, formatDroppedPaths, quoteDroppedPath } from "./drop";

describe("dropped paths", () => {
  it("single-quotes POSIX paths and escapes a quote inside", () => {
    expect(quoteDroppedPath("/a b/c.png", false)).toBe("'/a b/c.png'");
    expect(quoteDroppedPath("/it's.png", false)).toBe("'/it'\\''s.png'");
  });
  it("double-quotes Windows paths", () => {
    expect(quoteDroppedPath("C:\\My Files\\a.png", true)).toBe('"C:\\My Files\\a.png"');
  });
  it("joins several paths with a trailing space, and nothing for none", () => {
    expect(formatDroppedPaths(["/a", "/b c"], false)).toBe("'/a' '/b c' ");
    expect(formatDroppedPaths([], false)).toBe("");
  });
});

describe("drop position", () => {
  it("converts physical pixels to CSS pixels", () => {
    expect(dropPointToCss({ x: 200, y: 100 }, 2)).toEqual({ x: 100, y: 50 });
  });
  it("leaves a scale of 1 unchanged", () => {
    expect(dropPointToCss({ x: 200, y: 100 }, 1)).toEqual({ x: 200, y: 100 });
  });
  it("treats a scale of 0 or NaN as 1", () => {
    expect(dropPointToCss({ x: 8, y: 4 }, 0)).toEqual({ x: 8, y: 4 });
    expect(dropPointToCss({ x: 8, y: 4 }, Number.NaN)).toEqual({ x: 8, y: 4 });
  });
});
