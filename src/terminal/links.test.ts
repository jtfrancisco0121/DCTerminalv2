import { describe, expect, it } from "vitest";
import { isLinkModifier } from "./links";

describe("isLinkModifier", () => {
  it("is Cmd on macOS and Ctrl elsewhere", () => {
    expect(isLinkModifier({ metaKey: true, ctrlKey: false }, "mac")).toBe(true);
    expect(isLinkModifier({ metaKey: false, ctrlKey: true }, "mac")).toBe(false);
    expect(isLinkModifier({ metaKey: false, ctrlKey: true }, "linux")).toBe(true);
    expect(isLinkModifier({ metaKey: true, ctrlKey: false }, "windows")).toBe(false);
  });
});
