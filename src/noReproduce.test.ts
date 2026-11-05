import { describe, expect, it } from "vitest";

// The UI says "recorded", "re-run" and "new execution", never the word for pretending to get the same result again.
const sources = import.meta.glob("/src/**/*.{ts,tsx}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const WORD = new RegExp(["repro", "duce"].join(""), "i");

describe("replay words", () => {
  it("never uses the banned word in src (tests excluded)", () => {
    const files = Object.entries(sources).filter(([path]) => !/\.(test|cap\.test)\.tsx?$/.test(path) && !path.endsWith("noReproduce.test.ts"));
    expect(files.length).toBeGreaterThan(50);
    const hits = files.filter(([, text]) => WORD.test(text)).map(([path]) => path);
    expect(hits).toEqual([]);
  });
});
