// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PtyPacket } from "../bridge";

const handlers: ((packet: PtyPacket) => void)[] = [];
const ptyWrite = vi.fn<(id: string, data: string) => Promise<void>>();
const written: string[] = [];
const inputs: ((data: string) => void)[] = [];

vi.mock("../bridge", () => ({
  createPtyChannel: (onPacket: (packet: PtyPacket) => void) => {
    handlers.push(onPacket);
    return { onmessage: onPacket };
  },
  ptyOpen: vi.fn(() => Promise.resolve(1)),
  ptyWrite: (id: string, data: string) => ptyWrite(id, data),
  ptySetScheme: vi.fn(() => Promise.resolve()),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class {} }));
vi.mock("@xterm/addon-search", () => ({ SearchAddon: class {} }));
vi.mock("@xterm/addon-web-links", () => ({ WebLinksAddon: class {} }));
vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    options = {};
    loadAddon() {}
    onSelectionChange() {}
    onData(handler: (data: string) => void) {
      inputs.push(handler);
      return { dispose() {} };
    }
    write(bytes: Uint8Array) {
      written.push(new TextDecoder().decode(bytes));
    }
    dispose() {}
    open() {}
  },
}));

import { beginLivePty, dropLivePty } from "./live";
import { ensureParkedTerminal, releaseParkedTerminal } from "./park";

const push = (text: string) => handlers[handlers.length - 1]({ kind: "data", data: btoa(text), code: null });

describe("a parked terminal owns its streams", () => {
  beforeEach(() => {
    ptyWrite.mockReset().mockResolvedValue(undefined);
  });
  afterEach(() => {
    releaseParkedTerminal("p1");
    dropLivePty("p1");
    handlers.length = 0;
    written.length = 0;
    inputs.length = 0;
  });

  it("receives data pushed while no view is mounted, in order", () => {
    beginLivePty("p1");
    push("early ");
    ensureParkedTerminal("p1", 13);
    push("late");
    expect(written.join("")).toBe("early late");
  });

  it("attaches to the live session that starts after it was created, and again after a restart", () => {
    ensureParkedTerminal("p1", 13);
    beginLivePty("p1");
    push("first");
    beginLivePty("p1");
    push("second");
    expect(written).toEqual(["first", "second"]);
  });

  it("sends keystrokes through live.ts and stops after release", () => {
    beginLivePty("p1");
    ensureParkedTerminal("p1", 13);
    inputs[0]("ls\r");
    expect(ptyWrite).toHaveBeenCalledWith("p1", "ls\r");
    releaseParkedTerminal("p1");
    push("after release");
    expect(written).toEqual([]);
  });
});
