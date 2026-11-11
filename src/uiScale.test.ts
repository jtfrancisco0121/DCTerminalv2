// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { applyUiScale, cachedUiScale, normalizeUiScale, UI_SCALE_EVENT, UI_SCALES } from "./uiScale";

afterEach(() => {
  localStorage.clear();
  document.documentElement.style.fontSize = "";
  vi.restoreAllMocks();
});

describe("UI scale", () => {
  it("offers 100 and keeps the list sorted", () => {
    expect(UI_SCALES).toContain(100);
    expect([...UI_SCALES].sort((a, b) => a - b)).toEqual([...UI_SCALES]);
  });

  it("turns an unknown or junk value into 100", () => {
    expect(normalizeUiScale(110)).toBe(110);
    expect(normalizeUiScale("110")).toBe(110);
    for (const junk of [137, 0, -5, "abc", "", null, undefined, NaN, {}, "110%"]) {
      expect(normalizeUiScale(junk), String(junk)).toBe(100);
    }
  });

  it("sets the root font size as a percent, caches it and announces it", () => {
    const seen: number[] = [];
    window.addEventListener(UI_SCALE_EVENT, (event) => seen.push((event as CustomEvent<number>).detail), { once: true });
    expect(applyUiScale(110)).toBe(110);
    expect(document.documentElement.style.fontSize).toBe("110%");
    expect(localStorage.getItem("dcterminalv2.uiScale")).toBe("110");
    expect(cachedUiScale()).toBe(110);
    expect(seen).toEqual([110]);
  });

  it("applies 100 for an unlisted value", () => {
    expect(applyUiScale(133)).toBe(100);
    expect(document.documentElement.style.fontSize).toBe("100%");
  });

  it("reads junk in the cache as 100", () => {
    localStorage.setItem("dcterminalv2.uiScale", "huge");
    expect(cachedUiScale()).toBe(100);
  });

  it("gives 100 when storage throws, and still applies the scale", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(cachedUiScale()).toBe(100);
    expect(applyUiScale(90)).toBe(90);
    expect(document.documentElement.style.fontSize).toBe("90%");
  });
});
