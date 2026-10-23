import { describe, expect, it } from "vitest";
import { MAX_INTAKE_REFS, parseIntakeRefs } from "./intakeRefs";

describe("parseIntakeRefs (C2)", () => {
  it("reads task links, custom-id links, list links and bare ids, mixed", () => {
    const text = [
      "https://app.clickup.com/t/86b1aa001",
      "https://app.clickup.com/t/9012345678/QA-129?comment=1",
      "https://app.clickup.com/9012345678/v/li/901800112233",
      "https://app.clickup.com/9012345678/v/l/6-901800112244-1",
      "86b1aa002, #86b1aa003  QA-131",
    ].join("\n");
    expect(parseIntakeRefs(text)).toEqual({
      refs: [
        { kind: "task", id: "86b1aa001", line: 1 },
        { kind: "task", id: "QA-129", teamId: "9012345678", custom: true, line: 2 },
        { kind: "list", id: "901800112233", line: 3 },
        { kind: "list", id: "901800112244", line: 4 },
        { kind: "task", id: "86b1aa002", line: 5 },
        { kind: "task", id: "86b1aa003", line: 5 },
        { kind: "task", id: "QA-131", custom: true, line: 5 },
      ],
      problems: [],
    });
  });

  it("names each line it cannot read and keeps the rest", () => {
    const text = [
      "https://app.clickup.com/t/86b1aa001",
      "",
      "https://github.com/acme/repo/pull/12",
      "see the epic",
      "https://app.clickup.com/9012345678/home",
      "https://app.clickup.com/t/a%2Fb",
    ].join("\n");
    const parsed = parseIntakeRefs(text);
    expect(parsed.refs).toEqual([{ kind: "task", id: "86b1aa001", line: 1 }]);
    expect(parsed.problems.map((p) => p.message)).toEqual([
      "Line 3 is not a ClickUp task or list link.",
      "Line 4 is not a ClickUp task or list link.",
      "Line 5 is not a ClickUp task or list link.",
      "Line 6 is not a ClickUp task or list link.",
    ]);
  });

  it("lists a ref once and trims what a chat paste wraps it in", () => {
    const text = "<https://app.clickup.com/t/86b1aa001>\n(86b1aa001).\nhttps://app.clickup.com/t/86b1aa001";
    expect(parseIntakeRefs(text)).toEqual({
      refs: [{ kind: "task", id: "86b1aa001", line: 1 }],
      problems: [],
    });
  });

  it("refuses ids that could change a request path and plain words", () => {
    for (const bad of ["../x", "a?b", "hello-world", "epic", "ab12", "https://evil.example/t/86b1aa001"]) {
      expect(parseIntakeRefs(bad).refs).toEqual([]);
    }
  });

  it("caps a paste at the most the core reads", () => {
    const text = Array.from({ length: MAX_INTAKE_REFS + 2 }, (_, n) => `86b1a${n}`).join("\n");
    const parsed = parseIntakeRefs(text);
    expect(parsed.refs).toHaveLength(MAX_INTAKE_REFS);
    expect(parsed.problems).toEqual([
      { line: MAX_INTAKE_REFS + 1, message: `Paste at most ${MAX_INTAKE_REFS} links at once; line ${MAX_INTAKE_REFS + 1} is one too many.` },
    ]);
  });

  it("reads Markdown and Slack links by their url, not their text", () => {
    const text = [
      "See [QA-129 sync stalls](https://app.clickup.com/t/86b1aa001) and [the list](https://app.clickup.com/9012345678/v/li/901800112233).",
      "<https://app.clickup.com/t/86b1aa002|Duplicate log lines>",
    ].join("\n");
    const parsed = parseIntakeRefs(text);
    expect(parsed.refs).toEqual([
      { kind: "task", id: "86b1aa001", line: 1 },
      { kind: "list", id: "901800112233", line: 1 },
      { kind: "task", id: "86b1aa002", line: 2 },
    ]);
    // "See" and "and" are words on line 1, so it says so; the links still read.
    expect(parsed.problems).toEqual([{ line: 1, message: "Line 1 is not a ClickUp task or list link." }]);
    expect(parseIntakeRefs("[ticket](https://app.clickup.com/t/86b1aa001)")).toEqual({
      refs: [{ kind: "task", id: "86b1aa001", line: 1 }],
      problems: [],
    });
  });

  it("reads nothing from an empty box", () => {
    expect(parseIntakeRefs("  \n\n")).toEqual({ refs: [], problems: [] });
  });
});
