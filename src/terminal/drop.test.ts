import { describe, expect, it } from "vitest";
import { formatDroppedPaths, quoteDroppedPath } from "./drop";

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
