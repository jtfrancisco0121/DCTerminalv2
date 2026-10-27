import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PtyPacket } from "../bridge";

const handlers: ((packet: PtyPacket) => void)[] = [];
const ptyOpen = vi.fn<(input: unknown) => Promise<number>>();
const ptyWrite = vi.fn<(id: string, data: string) => Promise<void>>();
vi.mock("../bridge", () => ({
  createPtyChannel: (onPacket: (packet: PtyPacket) => void) => {
    handlers.push(onPacket);
    return { onmessage: onPacket };
  },
  ptyOpen: (input: unknown) => ptyOpen(input),
  ptyWrite: (id: string, data: string) => ptyWrite(id, data),
}));

const { beginLivePty, dropLivePty, livePty, openLivePty, ptyExitCode, ptyRunning, subscribePtyExit, writeLivePty } =
  await import("./live");

const last = () => handlers[handlers.length - 1];
const exit = (code: number | null) => last()({ kind: "exit", data: "", code });

const plan = { start: () => ({ cols: 80, rows: 24, resume: false }) };

/** A promise settled from outside, for a `ptyOpen` that stays pending. */
function deferred<T = void>() {
  let resolve!: (value: T) => void;
  let reject!: (err: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  ptyOpen.mockReset().mockResolvedValue(1);
  ptyWrite.mockReset().mockResolvedValue(undefined);
});

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
  it("opens a first terminal only once per page", async () => {
    const pending = deferred<number>();
    ptyOpen.mockReturnValueOnce(pending.promise);
    const first = openLivePty("t1", { ...plan, ifIdle: true });
    expect(await openLivePty("t1", { ...plan, ifIdle: true })).toBe("superseded");
    pending.resolve(1);
    expect(await first).toBe("opened");
    expect(await openLivePty("t1", { ...plan, ifIdle: true })).toBe("superseded");
    expect(ptyOpen).toHaveBeenCalledTimes(1);
  });

  it("a restart starts a fresh record at the new process's start", async () => {
    const now = vi.spyOn(Date, "now").mockReturnValue(1_000);
    beginLivePty("t1");
    exit(1);
    now.mockReturnValue(5_000);
    expect(await openLivePty("t1", plan)).toBe("opened");
    const live = livePty("t1")!;
    expect(live.startedAt).toBe(5_000);
    expect(live.exited).toBe(false);
  });
});

describe("keystrokes while a PTY opens", () => {
  it("are sent after the open resolves, in order with later writes", async () => {
    const pending = deferred<number>();
    ptyOpen.mockReturnValueOnce(pending.promise);
    const opened = openLivePty("t1", plan);
    const early = [writeLivePty("t1", "a"), writeLivePty("t1", "b")];
    expect(ptyWrite).not.toHaveBeenCalled();
    pending.resolve(1);
    expect(await opened).toBe("opened");
    void writeLivePty("t1", "c");
    await Promise.all(early);
    expect(ptyWrite.mock.calls).toEqual([
      ["t1", "a"],
      ["t1", "b"],
      ["t1", "c"],
    ]);
  });

  it("are dropped, quietly, when the open fails", async () => {
    const pending = deferred<number>();
    ptyOpen.mockReturnValueOnce(pending.promise);
    const errors: string[] = [];
    const opened = openLivePty("t1", { ...plan, onError: (m) => errors.push(m) });
    const queued = writeLivePty("t1", "a");
    pending.reject(new Error("no folder"));
    expect(await opened).toBe("failed");
    await expect(queued).resolves.toBeUndefined();
    expect(ptyWrite).not.toHaveBeenCalled();
    expect(errors).toEqual(["no folder"]);
  });

  it("a newer open supersedes the older one's flush and error", async () => {
    const older = deferred<number>();
    ptyOpen.mockReturnValueOnce(older.promise);
    const errors: string[] = [];
    const first = openLivePty("t1", { ...plan, onError: (m) => errors.push(m) });
    void writeLivePty("t1", "x");
    const second = openLivePty("t1", plan);
    expect(await second).toBe("opened");
    older.reject(new Error("old open failed"));
    expect(await first).toBe("superseded");
    expect(errors).toEqual([]);
    // The queued key belonged to the id, and the newer open sent it once.
    expect(ptyWrite.mock.calls).toEqual([["t1", "x"]]);
  });

  it("go straight to the PTY when no open is pending", async () => {
    await writeLivePty("t1", "z");
    expect(ptyWrite).toHaveBeenCalledWith("t1", "z");
  });
});

describe("a first open whose tab closed meanwhile", () => {
  it("starts nothing when the tab closes during the before step", async () => {
    const before = deferred<boolean>();
    const opened = openLivePty("t1", { ...plan, before: () => before.promise, start: () => ({ cols: 1, rows: 1, resume: false }) });
    dropLivePty("t1");
    before.resolve(true);
    expect(await opened).toBe("closed");
    expect(ptyOpen).not.toHaveBeenCalled();
    expect(livePty("t1")).toBeUndefined();
  });

  it("a reopened tab with the same id is the only one that opens", async () => {
    const before = deferred<boolean>();
    const stale = openLivePty("t1", { ...plan, before: () => before.promise });
    dropLivePty("t1");
    const reopened = openLivePty("t1", plan);
    before.resolve(true);
    expect(await stale).toBe("superseded");
    expect(await reopened).toBe("opened");
    expect(ptyOpen).toHaveBeenCalledTimes(1);
  });
});
