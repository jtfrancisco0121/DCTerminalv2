import { describe, expect, it } from "vitest";

// `open_file_link` has one caller in src: openInEditor in terminal/park.ts (one editor setting, no editor storm).
const sources = import.meta.glob("/src/**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

describe("open_file_link", () => {
  it("is invoked only from terminal/park.ts (tests excluded)", () => {
    const files = Object.entries(sources).filter(([path]) => !/\.test\.tsx?$/.test(path));
    expect(files.length).toBeGreaterThan(50);
    const hits = files.filter(([, text]) => text.includes("open_file_link")).map(([path]) => path);
    expect(hits).toEqual(["/src/terminal/park.ts"]);
  });
});
