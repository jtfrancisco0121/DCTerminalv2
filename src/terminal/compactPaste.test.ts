import { describe, expect, it, vi } from "vitest";
import { estimateTokens, lineCount, onPasteOffer, pasteOrOffer, shouldOffer } from "./compactPaste";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i}`).join("\n");

describe("compact paste thresholds", () => {
  it("offers only above 200 lines or 16 KB, and only with the setting on", () => {
    expect(shouldOffer(lines(200), true)).toBe(false);
    expect(shouldOffer(lines(201), true)).toBe(true);
    expect(shouldOffer("x".repeat(16 * 1024), true)).toBe(false);
    expect(shouldOffer("x".repeat(16 * 1024 + 1), true)).toBe(true);
    expect(shouldOffer(lines(5000), false)).toBe(false);
  });

  it("counts lines and estimates tokens as characters over four", () => {
    expect(lineCount("a\nb\n")).toBe(2);
    expect(lineCount("a\nb")).toBe(2);
    expect(estimateTokens("abcde")).toBe(2);
  });
});

describe("pasting", () => {
  it("pastes a small text unchanged with no bar", () => {
    const paste = vi.fn();
    const seen = vi.fn();
    const off = onPasteOffer(seen);
    expect(pasteOrOffer("t1", lines(10), true, paste)).toBe(false);
    expect(paste).toHaveBeenCalledWith(lines(10));
    expect(seen).not.toHaveBeenCalled();
    off();
  });

  it("hands a big paste to the bar and pastes nothing until a choice", () => {
    const paste = vi.fn();
    const seen = vi.fn();
    const off = onPasteOffer(seen);
    expect(pasteOrOffer("t1", lines(300), true, paste)).toBe(true);
    expect(paste).not.toHaveBeenCalled();
    expect(seen).toHaveBeenCalledOnce();
    seen.mock.calls[0]![0].paste("as is");
    expect(paste).toHaveBeenCalledWith("as is");
    off();
  });

  it("pastes a big text as is when no bar is mounted or the offer is off", () => {
    const paste = vi.fn();
    expect(pasteOrOffer("t1", lines(300), true, paste)).toBe(false);
    expect(paste).toHaveBeenCalledWith(lines(300));
    const off = onPasteOffer(vi.fn());
    expect(pasteOrOffer("t1", lines(300), false, paste)).toBe(false);
    off();
  });
});
