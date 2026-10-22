import { describe, expect, it } from "vitest";
import { lastLines, planFileMentions, stripAnsi, TERMINAL_TAIL_LINES } from "./text";

describe("terminal text", () => {
  it("lists plan file names in the order they were last printed", () => {
    const out =
      "Wrote ~/.claude/plans/\u001B[1mbrave-otter.md\u001B[0m\n" +
      "see README.md and .cursor/plans/fix_1.plan.md.\n" +
      "Updated /Users/x/.claude/plans/brave-otter.md";
    expect(planFileMentions(out)).toEqual(["README.md", "fix_1.plan.md", "brave-otter.md"]);
    expect(planFileMentions("no files here, just .md")).toEqual([]);
  });

  it("strips color and cursor sequences", () => {
    const raw = "\u001B[32mplan\u001B[0m \u001B[1;31mready\u001B[m\r\n\u001B]0;title\u0007next";
    expect(stripAnsi(raw)).toBe("plan ready\r\nnext");
  });

  it("keeps the last N lines of a long buffer", () => {
    const lines = Array.from({ length: TERMINAL_TAIL_LINES + 5 }, (_, i) => `line ${i}`);
    const tail = lastLines(lines.join("\n"));
    expect(tail.startsWith(`line 5`)).toBe(true);
    expect(tail.endsWith(`line ${TERMINAL_TAIL_LINES + 4}`)).toBe(true);
    expect(tail.split("\n")).toHaveLength(TERMINAL_TAIL_LINES);
  });
});
