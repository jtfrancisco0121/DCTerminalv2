// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { pasteOrOffer } from "../terminal/compactPaste";
import { PasteBar } from "./PasteBar";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
afterEach(() => {
  cleanup();
  invoke.mockReset();
});

const big = Array.from({ length: 400 }, (_, i) => `compiling ${i}`).join("\n");

describe("PasteBar", () => {
  it("shows nothing for a small paste", () => {
    const paste = vi.fn();
    render(<PasteBar tabId="t1" />);
    pasteOrOffer("t1", "small", true, paste);
    expect(screen.queryByTestId("paste-bar")).toBeNull();
    expect(paste).toHaveBeenCalledWith("small");
  });

  it("pastes the original unless compacted is picked", async () => {
    invoke.mockResolvedValue({ text: "short", linesBefore: 400, linesAfter: 1 });
    const paste = vi.fn();
    render(<PasteBar tabId="t1" />);
    pasteOrOffer("t1", big, true, paste);
    expect(await screen.findByTestId("paste-bar")).toBeTruthy();
    expect(paste).not.toHaveBeenCalled();
    await waitFor(() => expect((screen.getByText("Paste compacted") as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByText("Paste as is"));
    expect(paste).toHaveBeenCalledWith(big);
    expect(screen.queryByTestId("paste-bar")).toBeNull();
  });

  it("pastes the compacted text when picked, and the original on Esc", async () => {
    invoke.mockResolvedValue({ text: "short", linesBefore: 400, linesAfter: 1 });
    const paste = vi.fn();
    render(<PasteBar tabId="t1" />);
    pasteOrOffer("t1", big, true, paste);
    await screen.findByTestId("paste-bar");
    await waitFor(() => expect((screen.getByText("Paste compacted") as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByText("Paste compacted"));
    expect(paste).toHaveBeenCalledWith("short");
    paste.mockClear();
    pasteOrOffer("t1", big, true, paste);
    fireEvent.keyDown(await screen.findByTestId("paste-bar"), { key: "Escape" });
    expect(paste).toHaveBeenCalledWith(big);
  });
});
