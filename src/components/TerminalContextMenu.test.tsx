// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TerminalContextMenu } from "./TerminalContextMenu";

afterEach(cleanup);

describe("TerminalContextMenu", () => {
  const items = (copyDisabled: boolean) => [
    { id: "copy", label: "Copy", shortcut: "Ctrl+Shift+C", disabled: copyDisabled, onSelect: vi.fn() },
    { id: "paste", label: "Paste", onSelect: vi.fn() },
    { id: "clear", label: "Clear scrollback", divider: true, onSelect: vi.fn() },
  ];

  it("focuses the first enabled item and moves with the arrow keys", () => {
    render(<TerminalContextMenu x={10} y={10} items={items(true)} />);
    const [copy, paste, clear] = screen.getAllByRole("menuitem") as HTMLButtonElement[];
    expect(copy.disabled).toBe(true);
    expect(document.activeElement).toBe(paste);
    fireEvent.keyDown(paste, { key: "ArrowDown" });
    expect(document.activeElement).toBe(clear);
    fireEvent.keyDown(clear, { key: "ArrowDown" });
    expect(document.activeElement).toBe(paste);
    fireEvent.keyDown(paste, { key: "End" });
    expect(document.activeElement).toBe(clear);
  });

  it("shows shortcuts and enables Copy with a selection", () => {
    render(<TerminalContextMenu x={10} y={10} items={items(false)} />);
    expect(screen.getByText("Ctrl+Shift+C")).toBeTruthy();
    const copy = screen.getByRole("menuitem", { name: /Copy/ }) as HTMLButtonElement;
    expect(copy.disabled).toBe(false);
    expect(document.activeElement).toBe(copy);
  });
});
