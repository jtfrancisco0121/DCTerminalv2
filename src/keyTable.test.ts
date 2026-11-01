import { describe, expect, it } from "vitest";
import { shortcutLabel, SHORTCUT_SAMPLES } from "./keyTable";
import { isScratchpadShortcut } from "./terminal/keys";

describe("shortcutLabel", () => {
  it("keeps the macOS glyph tooltips", () => {
    expect(shortcutLabel("new-window", "mac")).toBe("⌘⇧N");
    expect(shortcutLabel("settings", "mac")).toBe("⌘,");
    expect(shortcutLabel("scratchpad", "mac", "text")).toBe("Cmd+Shift+E");
  });

  it("reads Ctrl+Shift+<key> on Windows and Linux", () => {
    expect(shortcutLabel("new-window", "windows")).toBe("Ctrl+Shift+N");
    expect(shortcutLabel("settings", "windows")).toBe("Ctrl+Shift+,");
    expect(shortcutLabel("scratchpad", "windows", "text")).toBe("Ctrl+Shift+E");
    expect(shortcutLabel("scratchpad", "linux")).toBe("Ctrl+Shift+E");
  });

  it("shows no label for a key with no binding on the platform", () => {
    expect(shortcutLabel("reopen-closed-tab", "windows")).toBe("");
    expect(shortcutLabel("reopen-closed-tab", "mac")).toBe("⌘⇧T");
  });

  it("matches the scratchpad key the terminal listens to", () => {
    const sample = SHORTCUT_SAMPLES.find((s) => s.action.kind === "scratchpad")!;
    expect(isScratchpadShortcut({ type: "keydown", ...sample.mac }, "mac")).toBe(true);
    expect(isScratchpadShortcut({ type: "keydown", ...sample.other! }, "windows")).toBe(true);
  });
});
