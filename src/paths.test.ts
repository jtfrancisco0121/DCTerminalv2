import { describe, expect, it } from "vitest";
import { baseName, dirName, splitAbsolute } from "./paths";
import { resolveRelative } from "./preview/bus";

describe("path helpers", () => {
  it("baseName accepts both separators and keeps a drive root", () => {
    expect(baseName("C:\\a\\b.rs")).toBe("b.rs");
    expect(baseName("C:/a/b.rs")).toBe("b.rs");
    expect(baseName("C:\\")).toBe("C:\\");
    expect(baseName("/a/b")).toBe("b");
    expect(baseName("/a/b/")).toBe("b");
    expect(baseName("/")).toBe("");
    expect(baseName("plain.md")).toBe("plain.md");
    expect(baseName("")).toBe("");
  });

  it("dirName keeps a drive root and the POSIX behaviour", () => {
    expect(dirName("C:\\a\\b.rs")).toBe("C:\\a");
    expect(dirName("C:\\a")).toBe("C:\\");
    expect(dirName("C:\\")).toBe("C:\\");
    expect(dirName("C:/a/b.rs")).toBe("C:/a");
    expect(dirName("/a/b")).toBe("/a");
    expect(dirName("/a")).toBe("/");
    expect(dirName("/")).toBe("/");
  });

  it("splits an absolute path into root, separator and names", () => {
    expect(splitAbsolute("C:\\x\\y")).toEqual({ root: "C:\\", sep: "\\", parts: ["x", "y"] });
    expect(splitAbsolute("/x/y")).toEqual({ root: "/", sep: "/", parts: ["x", "y"] });
  });

  it("resolves a relative link against a Windows or POSIX file", () => {
    expect(resolveRelative("C:\\docs\\a\\plan.md", "../b/c.md")).toBe("C:\\docs\\b\\c.md");
    expect(resolveRelative("C:\\plan.md", "c.md")).toBe("C:\\c.md");
    expect(resolveRelative("/docs/a/plan.md", "../b/c.md")).toBe("/docs/b/c.md");
    expect(resolveRelative("/plan.md", "c.md")).toBe("/c.md");
    expect(resolveRelative("C:\\docs\\plan.md", "https://x.y/z")).toBeNull();
  });
});
