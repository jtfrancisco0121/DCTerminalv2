/**
 * Asks the backend which printed paths exist (`paths_exist`), at most 50 per call, and remembers the
 * answers per tab for 10 seconds (at most 500 entries per tab), so hovering a line twice asks once.
 */
import { invoke } from "@tauri-apps/api/core";

export type PathHit = { candidate: string; path: string | null; previewable: boolean };
export type PathsExist = (tabId: string, candidates: string[]) => Promise<PathHit[]>;

export const TTL_MS = 10_000;
export const MAX_ENTRIES = 500;
export const BATCH = 50;

const ask: PathsExist = (tabId, candidates) => invoke<PathHit[]>("paths_exist", { tabId, candidates });

export class PathCache {
  private tabs = new Map<string, Map<string, { hit: PathHit; at: number }>>();
  calls = 0;

  constructor(
    private readonly exist: PathsExist = ask,
    private readonly now: () => number = () => Date.now(),
  ) {}

  /** The answer for each candidate (a candidate the backend did not answer is not linked). */
  async resolve(tabId: string, candidates: string[]): Promise<Map<string, PathHit>> {
    const cache = this.tabs.get(tabId) ?? new Map();
    this.tabs.set(tabId, cache);
    const t = this.now();
    const unique = [...new Set(candidates)];
    const missing = unique.filter((c) => {
      const e = cache.get(c);
      return !e || t - e.at > TTL_MS;
    });
    for (let i = 0; i < missing.length; i += BATCH) {
      this.calls += 1;
      const hits = await this.exist(tabId, missing.slice(i, i + BATCH));
      for (const hit of hits) {
        cache.delete(hit.candidate);
        cache.set(hit.candidate, { hit, at: t });
      }
    }
    while (cache.size > MAX_ENTRIES) cache.delete(cache.keys().next().value as string);
    const out = new Map<string, PathHit>();
    for (const c of unique) {
      const e = cache.get(c);
      if (e) out.set(c, e.hit);
    }
    return out;
  }

  forget(tabId: string): void {
    this.tabs.delete(tabId);
  }
}
