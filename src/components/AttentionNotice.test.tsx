// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttentionEvent } from "./AttentionNotice";

let handler: ((event: { payload: AttentionEvent }) => void) | null = null;
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_name: string, fn: (event: { payload: AttentionEvent }) => void) => {
    handler = fn;
    return () => {
      handler = null;
    };
  }),
}));
const focusTab = vi.fn(async (_id: string) => null);
vi.mock("../bridge", () => ({ focusTab: (id: string) => focusTab(id) }));

const { AttentionNotices, MAX_NOTICES, NOTICE_MS } = await import("./AttentionNotice");

const event = (n: number, tabId: string | null = `t${n}`): AttentionEvent => ({
  kind: "needs-you",
  tabId,
  windowLabel: "main",
  runId: null,
  text: `dcv2: tab ${n} needs you`,
});

const send = async (e: AttentionEvent) => {
  await act(async () => handler!({ payload: e }));
};

describe("AttentionNotices", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });
  afterEach(() => {
    vi.useRealTimers();
    focusTab.mockClear();
  });

  it("shows the event's words, Go to tab calls the bridge, Dismiss removes it", async () => {
    render(<AttentionNotices />);
    await act(async () => {});
    await send(event(1));
    expect(screen.getByText("dcv2: tab 1 needs you")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Go to tab" }));
    expect(focusTab).toHaveBeenCalledWith("t1");
    expect(screen.queryByText("dcv2: tab 1 needs you")).toBeNull();
    await send(event(2, null));
    expect(screen.queryByRole("button", { name: "Go to tab" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("dcv2: tab 2 needs you")).toBeNull();
  });

  it("keeps at most three and goes by itself after 15 s", async () => {
    render(<AttentionNotices />);
    await act(async () => {});
    for (const n of [1, 2, 3, 4]) await send(event(n));
    expect(MAX_NOTICES).toBe(3);
    expect(screen.queryByText("dcv2: tab 1 needs you")).toBeNull();
    expect(screen.getAllByRole("status")).toHaveLength(3);
    await act(async () => {
      vi.advanceTimersByTime(NOTICE_MS + 10);
    });
    expect(screen.queryAllByRole("status")).toHaveLength(0);
  });
});
