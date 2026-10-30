import { describe, expect, it, vi } from "vitest";
import { BATCH, MAX_ENTRIES, PathCache, TTL_MS, type PathHit } from "./fileLinkCache";

const answer = (cs: string[]): PathHit[] => cs.map((c) => ({ candidate: c, path: c.startsWith("no") ? null : `/w/${c}`, previewable: false }));

describe("PathCache", () => {
  it("asks once for repeated candidates and keeps answers for 10 s", async () => {
    let now = 1000;
    const exist = vi.fn(async (_t: string, cs: string[]) => answer(cs));
    const cache = new PathCache(exist, () => now);
    await cache.resolve("t1", ["a.ts", "b.ts"]);
    await cache.resolve("t1", ["a.ts"]);
    expect(exist).toHaveBeenCalledTimes(1);
    now += TTL_MS + 1;
    await cache.resolve("t1", ["a.ts"]);
    expect(exist).toHaveBeenCalledTimes(2);
  });

  it("keeps one cache per tab and returns the answer for each candidate", async () => {
    const exist = vi.fn(async (_t: string, cs: string[]) => answer(cs));
    const cache = new PathCache(exist);
    const a = await cache.resolve("t1", ["x.ts", "no.ts"]);
    expect(a.get("x.ts")?.path).toBe("/w/x.ts");
    expect(a.get("no.ts")?.path).toBeNull();
    await cache.resolve("t2", ["x.ts"]);
    expect(exist).toHaveBeenCalledTimes(2);
  });

  it("batches at most 50 candidates a call", async () => {
    const exist = vi.fn(async (_t: string, cs: string[]) => answer(cs));
    const cache = new PathCache(exist);
    await cache.resolve("t1", Array.from({ length: 120 }, (_, i) => `f${i}.ts`));
    expect(exist.mock.calls.map((c) => c[1].length)).toEqual([BATCH, BATCH, 20]);
  });

  it("keeps at most 500 entries a tab", async () => {
    const exist = vi.fn(async (_t: string, cs: string[]) => answer(cs));
    const cache = new PathCache(exist);
    await cache.resolve("t1", Array.from({ length: MAX_ENTRIES + 60 }, (_, i) => `f${i}.ts`));
    await cache.resolve("t1", ["f0.ts"]);
    expect(exist.mock.calls.length).toBe(Math.ceil((MAX_ENTRIES + 60) / BATCH) + 1);
  });
});
