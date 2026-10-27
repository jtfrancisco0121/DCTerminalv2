// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { ResolvedClaudeAccount } from "../bridge";
import { AccountPicker } from "./AccountPicker";

const accounts = [
  { id: "default", name: "Personal", config: { display: "~/.claude" }, envOverride: false },
] as unknown as ResolvedClaudeAccount[];

function picker(onOpen: (id: string) => Promise<void>) {
  const onClose = vi.fn();
  render(<AccountPicker accounts={accounts} current="default" onOpen={onOpen} onClose={onClose} />);
  return { onClose, button: () => screen.getByRole("button", { name: "Open window" }) as HTMLButtonElement };
}

describe("AccountPicker submit", () => {
  it("a double click opens one window and the button is disabled while pending", async () => {
    let finish!: () => void;
    const onOpen = vi.fn(() => new Promise<void>((resolve) => (finish = resolve)));
    const { button, onClose } = picker(onOpen);
    fireEvent.click(button());
    fireEvent.click(button());
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(button().disabled).toBe(true);
    await act(async () => finish());
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("after a rejection the button is enabled again and the error shows", async () => {
    const onOpen = vi.fn(() => Promise.reject(new Error("no such account")));
    const { button, onClose } = picker(onOpen);
    await act(async () => {
      fireEvent.click(button());
    });
    expect(screen.getByRole("alert").textContent).toContain("no such account");
    expect(button().disabled).toBe(false);
    expect(onClose).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(button());
    });
    expect(onOpen).toHaveBeenCalledTimes(2);
  });
});
