import { describe, expect, it } from "vitest";
import { createPtyBuffer, decodeBase64, MAX_PENDING_BYTES } from "./buffer";

describe("pty buffer", () => {
  it("replays bytes that arrived before xterm attached", () => {
    const buffer = createPtyBuffer();
    buffer.pushData(Uint8Array.from([65, 66]));
    buffer.pushExit(0);
    const seen: number[] = [];
    let code: number | null = null;
    buffer.attach({
      data: (bytes) => seen.push(...bytes),
      exit: (value) => {
        code = value;
      },
    });
    expect(seen).toEqual([65, 66]);
    expect(code).toBe(0);
  });

  it("keeps at most the cap while nothing is attached, and the newest bytes", () => {
    const buffer = createPtyBuffer();
    const chunk = 100 * 1024;
    const count = Math.ceil(MAX_PENDING_BYTES / chunk) + 5;
    for (let i = 0; i < count; i += 1) buffer.pushData(new Uint8Array(chunk).fill(i % 250));
    // One more byte pushes the oldest kept chunk to be cut, not only dropped whole.
    buffer.pushData(Uint8Array.from([251]));
    const seen: Uint8Array[] = [];
    buffer.attach({ data: (bytes) => seen.push(bytes), exit: () => {} });
    const total = seen.reduce((sum, bytes) => sum + bytes.length, 0);
    expect(total).toBe(MAX_PENDING_BYTES);
    expect(seen[seen.length - 1][0]).toBe(251);
    expect(seen[seen.length - 2][0]).toBe((count - 1) % 250);
    // In order: the values never go down across chunks, but for the final marker.
    const firsts = seen.slice(0, -1).map((bytes) => bytes[0]);
    expect(firsts).toEqual([...firsts].sort((a, b) => a - b));
  });

  it("does not hold bytes once attached", () => {
    const buffer = createPtyBuffer();
    const seen: number[] = [];
    buffer.attach({ data: (bytes) => seen.push(...bytes), exit: () => {} });
    buffer.pushData(Uint8Array.from([1, 2]));
    expect(seen).toEqual([1, 2]);
  });

  it("decodes base64 PTY payloads", () => {
    expect(Array.from(decodeBase64(btoa("ok")))).toEqual([111, 107]);
  });
});
