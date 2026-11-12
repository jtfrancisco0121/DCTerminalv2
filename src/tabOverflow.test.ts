import { describe, expect, it } from "vitest";
import { hiddenTabs } from "./tabOverflow";

const view = { left: 100, right: 400 };
const box = (id: string, left: number, right: number) => ({ id, left, right });

describe("hiddenTabs", () => {
  it("hides nothing when every tab is fully inside", () => {
    expect(hiddenTabs([box("a", 100, 200), box("b", 200, 400)], view)).toEqual([]);
  });
  it("hides a tab cut on the left or on the right, in tab order", () => {
    expect(hiddenTabs([box("a", 90, 190), box("b", 190, 290), box("c", 290, 410)], view)).toEqual(["a", "c"]);
  });
  it("hides a tab fully outside either edge", () => {
    expect(hiddenTabs([box("a", 0, 90), box("b", 100, 300), box("c", 410, 500)], view)).toEqual(["a", "c"]);
  });
  it("hides every tab when none is inside, and returns an empty list for no tabs", () => {
    expect(hiddenTabs([box("a", 0, 50), box("b", 450, 500)], view)).toEqual(["a", "b"]);
    expect(hiddenTabs([], view)).toEqual([]);
  });
});
