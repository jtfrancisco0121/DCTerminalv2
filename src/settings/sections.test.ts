import { describe, expect, it } from "vitest";
import { readSource } from "../readSource";
import { currentSection, matchesFilter, SETTINGS_SECTIONS } from "./sections";

describe("currentSection", () => {
  const tops = [0, 400, 900, 1500];

  it("is the first section at the top of the page", () => {
    expect(currentSection(tops, 0)).toBe(0);
  });

  it("is the last section whose top has reached the top of the page", () => {
    expect(currentSection(tops, 380)).toBe(1);
    expect(currentSection(tops, 400)).toBe(1);
    expect(currentSection(tops, 1499)).toBe(3);
    expect(currentSection(tops, 99999)).toBe(3);
  });

  it("skips sections that are not laid out", () => {
    expect(currentSection([0, Number.POSITIVE_INFINITY, 900], 1000)).toBe(2);
    expect(currentSection([Number.POSITIVE_INFINITY], 0)).toBe(0);
  });
});

describe("matchesFilter", () => {
  it("matches every word, ignoring case", () => {
    expect(matchesFilter("Theme: Soft Dusk", "theme")).toBe(true);
    expect(matchesFilter("Theme: Soft Dusk", "SOFT dusk")).toBe(true);
    expect(matchesFilter("Theme: Soft Dusk", "font")).toBe(false);
    expect(matchesFilter("anything", "  ")).toBe(true);
  });
});

describe("the one section list", () => {
  it("is used by the palette and the page, not copied", () => {
    expect(readSource("App.tsx")).toMatch(/import \{ SETTINGS_SECTIONS \} from "\.\/settings\/sections"/);
    expect(readSource("App.tsx")).not.toMatch(/const SETTINGS_SECTIONS/);
    expect(readSource("components/SettingsPage.tsx")).toMatch(/SETTINGS_SECTIONS.*from "\.\.\/settings\/sections"/);
    expect(new Set(SETTINGS_SECTIONS).size).toBe(SETTINGS_SECTIONS.length);
  });
});
