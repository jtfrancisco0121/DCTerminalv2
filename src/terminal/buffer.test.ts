import { describe, expect, it } from "vitest";
import { createPtyBuffer, decodeBase64 } from "./buffer";

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

  it("decodes base64 PTY payloads", () => {
    expect(Array.from(decodeBase64(btoa("ok")))).toEqual([111, 107]);
  });
});
