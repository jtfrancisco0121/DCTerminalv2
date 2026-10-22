import { afterEach, describe, expect, it, vi } from "vitest";
import type { PtyPacket } from "../bridge";

const handlers: ((packet: PtyPacket) => void)[] = [];
vi.mock("../bridge", () => ({
  createPtyChannel: (onPacket: (packet: PtyPacket) => void) => {
    handlers.push(onPacket);
    return { onmessage: onPacket };
  },
}));

const { beginLivePty, dropLivePty, livePty, markPtyOpening, ptyExitCode, ptyRunning, restartLivePty, subscribePtyExit } =
  await import("./live");

const last = () => handlers[handlers.length - 1];
const exit = (code: number | null) => last()({ kind: "exit", data: "", code });

afterEach(() => {
  dropLivePty("t1");
  handlers.length = 0;
  vi.restoreAllMocks();
});

describe("live PTY exit", () => {
  it("tells exit listeners once, with -1 for a signal", () => {
    const seen: [string, number][] = [];
    const off = subscribePtyExit((id, code) => seen.push([id, code]));
    beginLivePty("t1");
    expect(ptyExitCode("t1")).toBeNull();
    expect(ptyRunning("t1")).toBe(true);
    exit(3);
    expect(seen).toEqual([["t1", 3]]);
    expect(ptyExitCode("t1")).toBe(3);
    expect(ptyRunning("t1")).toBe(false);
    beginLivePty("t1");
    exit(null);
    expect(seen).toEqual([["t1", 3], ["t1", -1]]);
    expect(ptyExitCode("t1")).toBe(-1);
    off();
  });

  it("is silent for a closed tab or a replaced process", () => {
    const seen: string[] = [];
    const off = subscribePtyExit((id) => seen.push(id));
    beginLivePty("t1");
    const first = last();
    beginLivePty("t1");
    first({ kind: "exit", data: "", code: 1 });
    dropLivePty("t1");
    exit(1);
    expect(seen).toEqual([]);
    off();
  });

  it("buffers output until xterm attaches", () => {
    const live = beginLivePty("t1");
    last()({ kind: "data", data: btoa("hi"), code: null });
    const seen: number[] = [];
    live.buffer.attach({ data: (bytes) => seen.push(...bytes), exit: () => {} });
    expect(String.fromCharCode(...seen)).toBe("hi");
  });
});

describe("opening and restart", () => {
  it("opens a terminal only once per page", () => {
    expect(markPtyOpening("t1")).toBe(true);
    expect(markPtyOpening("t1")).toBe(false);
    beginLivePty("t1");
    expect(markPtyOpening("t1")).toBe(false);
  });

  it("a restart starts a fresh record at the new process's start", () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    beginLivePty("t1");
    exit(1);
    now.mockReturnValue(5_000);
    const live = restartLivePty("t1");
    expect(livePty("t1")).toBe(live);
    expect(live.startedAt).toBe(5_000);
    expect(live.exited).toBe(false);
  });
});
