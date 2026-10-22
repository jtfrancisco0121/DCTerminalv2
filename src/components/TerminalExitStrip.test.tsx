// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TerminalExitStrip, lastFiftyLines, lastOutputLine } from "./TerminalExitStrip";

const ID = "11111111-2222-4333-8444-555555555555";

function strip(over: Partial<React.ComponentProps<typeof TerminalExitStrip>> = {}) {
  const props = {
    exitCode: 1,
    lastLine: "Error: socket closed",
    failed: null,
    resumeId: ID as string | null | undefined,
    copied: false,
    onResume: vi.fn(),
    onFresh: vi.fn(),
    onCopy: vi.fn(),
    ...over,
  };
  render(<TerminalExitStrip {...props} />);
  return props;
}

describe("TerminalExitStrip (R18)", () => {
  it("shows the exit code, last line, and the three actions", () => {
    const props = strip();
    expect(screen.getByText("Process exited (1).")).toBeTruthy();
    expect(screen.getByText("Error: socket closed")).toBeTruthy();
    expect(screen.getByText(/Start fresh loses this conversation/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Resume session" }));
    fireEvent.click(screen.getByRole("button", { name: "Start fresh" }));
    fireEvent.click(screen.getByRole("button", { name: "Copy last 50 lines" }));
    expect(props.onResume).toHaveBeenCalledOnce();
    expect(props.onFresh).toHaveBeenCalledOnce();
    expect(props.onCopy).toHaveBeenCalledOnce();
  });

  it("without a saved session offers only Start fresh and Copy, with a reason", () => {
    strip({ resumeId: null });
    expect(screen.queryByRole("button", { name: "Resume session" })).toBeNull();
    expect(screen.getByText(/No saved Claude session/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Start fresh" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Copy last 50 lines" })).toBeTruthy();
  });

  it("disables Resume while it checks for the session", () => {
    strip({ resumeId: undefined });
    const resume = screen.getByRole("button", { name: "Resume session" }) as HTMLButtonElement;
    expect(resume.disabled).toBe(true);
  });

  it("reads the last output line and the last 50 lines", () => {
    expect(lastOutputLine("a\n\u001b[31mboom\u001b[0m\n\n   \n")).toBe("boom");
    expect(lastOutputLine("")).toBe("");
    const text = Array.from({ length: 80 }, (_, i) => `line ${i}`).join("\n");
    const out = lastFiftyLines(text).split("\n");
    expect(out).toHaveLength(50);
    expect(out[0]).toBe("line 30");
  });
});
