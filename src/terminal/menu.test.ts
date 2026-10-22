import { describe, expect, it } from "vitest";
import { clampMenuPosition, nextMenuIndex } from "./menu";

const size = { width: 200, height: 120 };
const viewport = { width: 800, height: 600 };

describe("clampMenuPosition", () => {
  it("leaves a menu that fits where it is", () => {
    expect(clampMenuPosition({ x: 100, y: 100 }, size, viewport)).toEqual({ x: 100, y: 100 });
  });
  it("pulls a menu back from the right and bottom edges", () => {
    expect(clampMenuPosition({ x: 790, y: 590 }, size, viewport)).toEqual({ x: 596, y: 476 });
  });
  it("never goes past the top-left, even in a tiny window", () => {
    expect(clampMenuPosition({ x: -5, y: -5 }, size, viewport)).toEqual({ x: 4, y: 4 });
    expect(clampMenuPosition({ x: 50, y: 50 }, size, { width: 100, height: 80 })).toEqual({
      x: 4,
      y: 4,
    });
  });
});

describe("nextMenuIndex", () => {
  it("skips disabled items and wraps", () => {
    const enabled = [false, true, true];
    expect(nextMenuIndex(-1, 1, enabled)).toBe(1);
    expect(nextMenuIndex(2, 1, enabled)).toBe(1);
    expect(nextMenuIndex(1, -1, enabled)).toBe(2);
  });
  it("returns -1 when nothing is enabled", () => {
    expect(nextMenuIndex(0, 1, [false, false])).toBe(-1);
  });
});
