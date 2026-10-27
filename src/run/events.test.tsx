// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearUiErrors, uiErrors } from "../uiErrors";

const handlers = new Map<string, () => void>();
const unlisten = vi.fn();
let listenFails = false;
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: () => void) => {
    if (listenFails) throw new Error("no event bridge");
    handlers.set(name, handler);
    return () => {
      handlers.delete(name);
      unlisten(name);
    };
  }),
}));

const { usePushRefresh, SLOW_MS } = await import("./events");

const refresh = vi.fn(async () => {});
function Probe({ names = "run-state" }: { names?: string | string[] }) {
  usePushRefresh(names, refresh, { fastMs: 2000 });
  return null;
}

const flush = () => act(async () => {});
const advance = (ms: number) => act(async () => void (await vi.advanceTimersByTimeAsync(ms)));

describe("usePushRefresh", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    refresh.mockClear();
    unlisten.mockClear();
    handlers.clear();
    listenFails = false;
    clearUiErrors();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("refreshes on mount, polls at the fast interval, and on an event", async () => {
    render(<Probe />);
    await flush();
    expect(refresh).toHaveBeenCalledTimes(1);
    await advance(2000);
    expect(refresh).toHaveBeenCalledTimes(2);
    await act(async () => handlers.get("run-state")?.());
    expect(refresh).toHaveBeenCalledTimes(3);
  });

  it("slows to the slow interval while events arrive and returns after three silent periods", async () => {
    render(<Probe />);
    await flush();
    await act(async () => handlers.get("run-state")?.());
    refresh.mockClear();
    await advance(SLOW_MS - 1);
    expect(refresh).not.toHaveBeenCalled();
    await advance(1);
    expect(refresh).toHaveBeenCalledTimes(1);
    // Silent for 3 periods: the next poll is at the fast interval again.
    await advance(2 * SLOW_MS);
    refresh.mockClear();
    await advance(2000);
    expect(refresh).toHaveBeenCalledTimes(1);
    await advance(2000);
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("keeps the fast interval when subscribing fails, and reports it once", async () => {
    listenFails = true;
    render(<Probe />);
    await flush();
    await advance(2000);
    await advance(2000);
    expect(refresh.mock.calls.length).toBeGreaterThanOrEqual(3);
    expect(uiErrors().filter((e) => e.source.startsWith("Live updates"))).toHaveLength(1);
  });

  it("refreshes on window focus and listens to every name, and unlistens on unmount", async () => {
    const { unmount } = render(<Probe names={["tab-state", "run-state"]} />);
    await flush();
    refresh.mockClear();
    act(() => void window.dispatchEvent(new Event("focus")));
    await flush();
    expect(refresh).toHaveBeenCalledTimes(1);
    await act(async () => handlers.get("tab-state")?.());
    expect(refresh).toHaveBeenCalledTimes(2);
    unmount();
    expect(unlisten).toHaveBeenCalledWith("tab-state");
    expect(unlisten).toHaveBeenCalledWith("run-state");
    refresh.mockClear();
    await advance(60_000);
    expect(refresh).not.toHaveBeenCalled();
  });
});
