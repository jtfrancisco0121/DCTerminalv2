// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clampWidth, DEFAULT_WIDTH_REM, layoutKey, loadLayout, MAX_WIDTH_REM, MIN_WIDTH_REM } from "../runPanelLayout";
import { RunPanelShell } from "./RunPanelShell";

function shell(props: Partial<React.ComponentProps<typeof RunPanelShell>> = {}) {
  return render(
    <RunPanelShell windowId="main" stageLabel="Building" attention={false} {...props}>
      <aside className="run-panel">panel body</aside>
    </RunPanelShell>,
  );
}

const width = () => Number(screen.getByRole("separator").getAttribute("aria-valuenow"));

describe("RunPanelShell", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("clamps the width to 16..40 rem", () => {
    expect(clampWidth(5)).toBe(MIN_WIDTH_REM);
    expect(clampWidth(99)).toBe(MAX_WIDTH_REM);
    expect(clampWidth(Number.NaN)).toBe(DEFAULT_WIDTH_REM);
    expect(clampWidth(25)).toBe(25);
  });

  it("starts at 20 rem; the arrow keys change it by 1 rem within the clamp; double-click resets", () => {
    shell();
    const handle = screen.getByRole("separator");
    expect(handle.getAttribute("aria-orientation")).toBe("vertical");
    expect(handle.getAttribute("aria-valuemin")).toBe("16");
    expect(handle.getAttribute("aria-valuemax")).toBe("40");
    expect(width()).toBe(20);
    fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(width()).toBe(21);
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(width()).toBe(19);
    for (let i = 0; i < 10; i++) fireEvent.keyDown(handle, { key: "ArrowRight" });
    expect(width()).toBe(16);
    for (let i = 0; i < 40; i++) fireEvent.keyDown(handle, { key: "ArrowLeft" });
    expect(width()).toBe(40);
    fireEvent.doubleClick(handle);
    expect(width()).toBe(20);
  });

  it("dragging the handle sets the width from the pointer, clamped", () => {
    shell();
    const handle = screen.getByRole("separator");
    const frame = handle.parentElement!;
    vi.spyOn(frame, "getBoundingClientRect").mockReturnValue({ right: 1000 } as DOMRect);
    // 16px per rem in jsdom's default.
    fireEvent.pointerDown(handle, { pointerId: 1, clientX: 680 });
    fireEvent.pointerMove(handle, { clientX: 680 });
    expect(width()).toBe(20);
    fireEvent.pointerMove(handle, { clientX: 560 });
    expect(width()).toBe(28); // 27.5 rem, shown rounded
    fireEvent.pointerMove(handle, { clientX: 100 });
    expect(width()).toBe(40);
    fireEvent.pointerUp(handle);
    fireEvent.pointerMove(handle, { clientX: 900 });
    expect(width()).toBe(40);
  });

  it("collapses to a rail and expands again; width and state persist per window", () => {
    const { unmount } = shell({ windowId: "win-2" });
    fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowLeft" });
    fireEvent.click(screen.getByRole("button", { name: "Collapse run panel" }));
    expect(screen.queryByText("panel body")).toBeNull();
    expect(JSON.parse(window.localStorage.getItem(layoutKey("win-2"))!)).toEqual({ width: 21, collapsed: true });
    unmount();
    // A reload in the same window finds the rail; another window does not.
    shell({ windowId: "win-2" });
    expect(screen.getByRole("button", { name: "Expand run panel" })).toBeTruthy();
    cleanup();
    shell({ windowId: "main" });
    expect(screen.getByText("panel body")).toBeTruthy();
    cleanup();
    shell({ windowId: "win-2" });
    fireEvent.click(screen.getByRole("button", { name: "Expand run panel" }));
    expect(width()).toBe(21);
  });

  it("a storage that throws does not break the render or the controls", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("full");
    });
    shell();
    expect(width()).toBe(20);
    fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowLeft" });
    expect(width()).toBe(21);
    expect(loadLayout("main")).toEqual({ width: 20, collapsed: false });
  });

  it("junk in storage reads as the default", () => {
    window.localStorage.setItem(layoutKey("main"), "{not json");
    shell();
    expect(width()).toBe(20);
    cleanup();
    window.localStorage.setItem(layoutKey("main"), JSON.stringify({ width: 999, collapsed: "yes" }));
    shell();
    expect(width()).toBe(40);
    expect(screen.getByText("panel body")).toBeTruthy();
  });

  it("the collapsed rail shows attention when the run waits for the user", () => {
    window.localStorage.setItem(layoutKey("main"), JSON.stringify({ width: 20, collapsed: true }));
    const { unmount } = shell({ attention: false });
    expect(screen.getByRole("button", { name: "Expand run panel" }).className).not.toContain("run-rail-attention");
    unmount();
    shell({ attention: true });
    const rail = screen.getByRole("button", { name: "Expand run panel" });
    expect(rail.className).toContain("run-rail-attention");
    expect(rail.title).toContain("waiting for you");
  });

  it("asks the terminal to refit after each change, not on mount", async () => {
    vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });
    const onLayoutChange = vi.fn();
    shell({ onLayoutChange });
    await act(async () => {
      vi.advanceTimersByTime(50);
    });
    expect(onLayoutChange).not.toHaveBeenCalled();
    fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowLeft" });
    await act(async () => {
      vi.advanceTimersByTime(50);
    });
    expect(onLayoutChange).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Collapse run panel" }));
    await act(async () => {
      vi.advanceTimersByTime(50);
    });
    expect(onLayoutChange).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });
});
