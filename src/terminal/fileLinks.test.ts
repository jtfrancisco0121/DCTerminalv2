import { describe, expect, it } from "vitest";
import { findFilePaths } from "./fileLinks";

const paths = (line: string) => findFilePaths(line).map((m) => m.path);
const at = (line: string, path: string) => findFilePaths(line).find((m) => m.path === path)!;

describe("Windows paths", () => {
  it("finds C:\\a\\b.rs:12 and C:/a/b.rs:12 as one link each, with their line", () => {
    expect(paths("error in C:\\work\\app\\main.rs:12 and C:/work/app/lib.rs:7:3 done")).toEqual([
      "C:\\work\\app\\main.rs",
      "C:/work/app/lib.rs",
    ]);
    const hit = at("see C:\\work\\main.rs:12", "C:\\work\\main.rs");
    expect([hit.line, hit.col, hit.start, hit.end]).toEqual([12, undefined, 4, 22]);
    const both = at("C:/w/lib.rs:7:3", "C:/w/lib.rs");
    expect([both.line, both.col]).toEqual([7, 3]);
  });

  it("does not take a URL or a bare drive letter for a path", () => {
    expect(paths("https://example.com/a/b.html and C: and C:\\")).toEqual([]);
  });
});

describe("file path matcher", () => {
  it("finds relative and absolute paths", () => {
    expect(paths("see docs/plan.md and ./src/a.ts and ../b/c.rs and /Users/me/x.json and ~/notes/todo.txt")).toEqual([
      "docs/plan.md",
      "./src/a.ts",
      "../b/c.rs",
      "/Users/me/x.json",
      "~/notes/todo.txt",
    ]);
    expect(paths("edit Cargo.toml now")).toEqual(["Cargo.toml"]);
  });

  it("reads file:line and file:line:col", () => {
    const m = at("error at src/main.rs:12:7: bad", "src/main.rs");
    expect([m.line, m.col]).toEqual([12, 7]);
    expect(at("src/main.rs:12 only", "src/main.rs").line).toBe(12);
    expect(at("src/main.rs:12 only", "src/main.rs").col).toBeUndefined();
    const line = "x src/a.ts:3:4 y";
    expect(line.slice(at(line, "src/a.ts").start, at(line, "src/a.ts").end)).toBe("src/a.ts:3:4");
  });

  it("finds the path inside a tool call", () => {
    expect(paths("● Write(docs/plan.md)")).toEqual(["docs/plan.md"]);
    expect(paths('● Read("src/My File.tsx")')).toEqual(["src/My File.tsx"]);
    expect(paths("Edit(src/a.ts, replace_all)")).toEqual(["src/a.ts"]);
    const line = "Write(docs/plan.md)";
    expect(line.slice(at(line, "docs/plan.md").start, at(line, "docs/plan.md").end)).toBe("docs/plan.md");
  });

  it("finds quoted paths, with spaces", () => {
    expect(paths('opened "/Users/me/My Docs/read me.md" ok')).toEqual(["/Users/me/My Docs/read me.md"]);
    expect(paths("saved 'out/final report.pdf'")).toEqual(["out/final report.pdf"]);
  });

  it("ignores URLs, version numbers, emails and plain words", () => {
    expect(paths("see https://example.com/a/b.html for more")).toEqual([]);
    expect(paths("version 1.2.3 and v0.7.0 and 3.14")).toEqual([]);
    expect(paths("mail me@example.com")).toEqual([]);
    expect(paths("nothing to see here, just words.")).toEqual([]);
  });

  it("does not report one range twice", () => {
    const ms = findFilePaths('Write(docs/plan.md) "docs/plan.md" docs/plan.md');
    for (const a of ms) for (const b of ms) if (a !== b) expect(a.start < b.end && a.end > b.start).toBe(false);
  });

  it("stays fast on a very long line", () => {
    const started = performance.now();
    findFilePaths(("src/a/b/c/d.ts ".repeat(2000)).slice(0, 30000));
    expect(performance.now() - started).toBeLessThan(500);
  });
});
