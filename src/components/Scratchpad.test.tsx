// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DraftStore, SAVE_DELAY_MS, type DraftsFile } from "../terminal/drafts";
import { Scratchpad } from "./Scratchpad";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const core = await import("@tauri-apps/api/core");
const invoke = vi.mocked(core.invoke);

beforeEach(() => {
  vi.useFakeTimers();
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const store = (file: DraftsFile = { drafts: {}, history: [] }) => new DraftStore(() => undefined, async () => file);

describe("Scratchpad", () => {
  it("saves after the debounce and restores the draft on reload", async () => {
    const s = store();
    await s.whenReady();
    render(<Scratchpad paneId="t1" store={s} bracketedPaste={() => true} onSend={vi.fn()} onClose={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Prompt draft"), { target: { value: "fix the retry" } });
    expect(invoke).not.toHaveBeenCalled();
    await act(async () => {
      vi.advanceTimersByTime(SAVE_DELAY_MS + 5);
    });
    expect(invoke).toHaveBeenCalledWith("drafts_save", { drafts: { t1: "fix the retry" }, history: [] });
    // A forced quit and relaunch: a new store loads what was saved.
    cleanup();
    const reloaded = store({ drafts: { t1: "fix the retry" }, history: [] });
    await reloaded.whenReady();
    render(<Scratchpad paneId="t1" store={reloaded} bracketedPaste={() => true} onSend={vi.fn()} onClose={vi.fn()} />);
    expect((screen.getByLabelText("Prompt draft") as HTMLTextAreaElement).value).toBe("fix the retry");
  });

  it("flushes on blur without waiting for the debounce", async () => {
    const s = store();
    await s.whenReady();
    render(<Scratchpad paneId="t1" store={s} bracketedPaste={() => true} onSend={vi.fn()} onClose={vi.fn()} />);
    const area = screen.getByLabelText("Prompt draft");
    fireEvent.change(area, { target: { value: "x" } });
    fireEvent.blur(area);
    await act(async () => {
      await Promise.resolve();
    });
    expect(invoke).toHaveBeenCalledWith("drafts_save", expect.objectContaining({ drafts: { t1: "x" } }));
  });

  it("sends exactly the draft text with nothing added", async () => {
    const s = store();
    await s.whenReady();
    const onSend = vi.fn();
    render(<Scratchpad paneId="t1" store={s} bracketedPaste={() => false} onSend={onSend} onClose={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Prompt draft"), { target: { value: "run the tests" } });
    fireEvent.click(screen.getByText("Send to terminal"));
    expect(onSend).toHaveBeenCalledOnce();
    expect(onSend).toHaveBeenCalledWith("run the tests");
    expect(onSend.mock.calls[0]![0]).not.toMatch(/[\r\n]/);
  });

  it("disables Send for any newline without bracketed paste, a trailing one included, and shows the reason", async () => {
    const s = store();
    await s.whenReady();
    const onSend = vi.fn();
    render(<Scratchpad paneId="t1" store={s} bracketedPaste={() => false} onSend={onSend} onClose={vi.fn()} />);
    const send = screen.getByText("Send to terminal") as HTMLButtonElement;
    fireEvent.change(screen.getByLabelText("Prompt draft"), { target: { value: "one line\n" } });
    expect(send.disabled).toBe(true);
    expect(screen.getByTestId("scratchpad-blocked").textContent).toContain("paste mode");
    fireEvent.click(send);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("allows a multi-line Send in bracketed paste mode, and hints never block it", async () => {
    const s = store();
    await s.whenReady();
    const onSend = vi.fn();
    render(<Scratchpad paneId="t1" store={s} bracketedPaste={() => true} onSend={onSend} onClose={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("Prompt draft"), { target: { value: "vague\nthing" } });
    expect(screen.getByTestId("scratchpad-hints")).toBeTruthy();
    const send = screen.getByText("Send to terminal") as HTMLButtonElement;
    expect(send.disabled).toBe(false);
    fireEvent.click(send);
    expect(onSend).toHaveBeenCalledWith("vague\nthing");
  });

  it("keeps one draft per pane", async () => {
    const s = store({ drafts: { a: "draft A", b: "draft B" }, history: [] });
    await s.whenReady();
    render(<Scratchpad paneId="b" store={s} bracketedPaste={() => true} onSend={vi.fn()} onClose={vi.fn()} />);
    expect((screen.getByLabelText("Prompt draft") as HTMLTextAreaElement).value).toBe("draft B");
  });
});
