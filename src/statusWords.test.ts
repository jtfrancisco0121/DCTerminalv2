import { describe, expect, it } from "vitest";

const sources = import.meta.glob("/src/components/{Stepper,EvalResults,StatusBar,TabBar}.tsx", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

// The status words live in src/run/words.ts; a component never spells them again.
describe("status words have one home", () => {
  it("finds the four components", () => {
    expect(Object.keys(sources)).toHaveLength(4);
  });
  for (const [path, text] of Object.entries(sources)) {
    it(`${path.split("/").pop()} has no "Failed" literal and defines no moved function`, () => {
      expect(text).not.toMatch(/["'>]Failed["'<]/);
      expect(text).not.toMatch(/function (attentionText|activityTitle)/);
    });
  }
});
