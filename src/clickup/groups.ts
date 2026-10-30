/** One run per ticket: how the kept tickets are split into runs (AD-13). */

export type SplitChoice = { perTicket: boolean; letters: Record<string, string> };

/**
 * The ticket ids of each run. All in one run unless `perTicket`; then tickets with the same group
 * letter share a run (letters are compared case-insensitively, in the order they first appear) and
 * every ticket without one gets its own.
 */
export function splitGroups(ids: string[], choice: SplitChoice): string[][] {
  if (!choice.perTicket) return ids.length ? [ids] : [];
  const out: string[][] = [];
  const byLetter = new Map<string, string[]>();
  for (const id of ids) {
    const letter = (choice.letters[id] ?? "").trim().toUpperCase().slice(0, 1);
    if (!letter) {
      out.push([id]);
    } else if (byLetter.has(letter)) {
      byLetter.get(letter)!.push(id);
    } else {
      const group = [id];
      byLetter.set(letter, group);
      out.push(group);
    }
  }
  return out;
}
