import { describe, expect, it } from "vitest";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const root = path.join(import.meta.dirname, "..");
const file = (name) => path.join(root, name);
const read = (name) => readFileSync(file(name), "utf8");

/** Local links `[text](target)` in a Markdown file, without anchors, URLs and mail links. */
function localLinks(markdown) {
  return [...markdown.matchAll(/\]\(([^)\s]+)\)/g)]
    .map((m) => m[1].split("#")[0])
    .filter((target) => target && !/^[a-z][a-z0-9+.-]*:/i.test(target));
}

describe("contributor files (AD-11)", () => {
  it("has CONTRIBUTING.md, SECURITY.md and the two issue forms", () => {
    for (const name of [
      "CONTRIBUTING.md",
      "SECURITY.md",
      ".github/ISSUE_TEMPLATE/bug_report.yml",
      ".github/ISSUE_TEMPLATE/feature_request.yml",
      ".github/ISSUE_TEMPLATE/config.yml",
    ]) {
      expect(existsSync(file(name)), name).toBe(true);
    }
  });

  it("links CONTRIBUTING.md and SECURITY.md from the README, and every local README link resolves", () => {
    const readme = read("README.md");
    const links = localLinks(readme);
    expect(links).toContain("CONTRIBUTING.md");
    expect(links).toContain("SECURITY.md");
    for (const target of links) {
      expect(existsSync(file(target)), `README links ${target}`).toBe(true);
    }
    for (const doc of ["CONTRIBUTING.md", "SECURITY.md"]) {
      for (const target of localLinks(read(doc))) {
        expect(existsSync(file(target)), `${doc} links ${target}`).toBe(true);
      }
    }
  });

  it("keeps .github to ISSUE_TEMPLATE/ only: no hosted CI workflows", () => {
    expect(readdirSync(file(".github")).sort()).toEqual(["ISSUE_TEMPLATE"]);
    expect(existsSync(file(".github/workflows"))).toBe(false);
    expect(readdirSync(file(".github/ISSUE_TEMPLATE")).sort()).toEqual([
      "bug_report.yml",
      "config.yml",
      "feature_request.yml",
    ]);
  });

  it("has issue forms made of plain key lines, and config.yml turns blank issues off", () => {
    for (const name of ["bug_report.yml", "feature_request.yml"]) {
      const text = read(`.github/ISSUE_TEMPLATE/${name}`);
      const lines = text.split("\n").filter((line) => line.trim() !== "");
      for (const line of lines) {
        expect(line, `${name}: ${line}`).toMatch(/^(\s*- )?\s*[A-Za-z_]+:(\s|$)|^\s+[^\s].*$/);
        expect(line.includes("\t"), `${name}: tab in ${line}`).toBe(false);
      }
      for (const key of ["name:", "description:", "body:", "- type: textarea", "id:", "label:"]) {
        expect(text, `${name} has ${key}`).toContain(key);
      }
      expect(text).toMatch(/required: true/);
    }
    expect(read(".github/ISSUE_TEMPLATE/config.yml")).toMatch(/^blank_issues_enabled: false$/m);
  });

  it("SECURITY.md points to private reporting and gives no personal email", () => {
    const text = read("SECURITY.md");
    expect(text).toMatch(/Report a vulnerability/);
    expect(text).toMatch(/latest release/);
    expect(text).not.toMatch(/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/);
  });

  it("explains the Receipt opt-in in the README", () => {
    const readme = read("README.md");
    expect(readme).toMatch(/^## Receipts$/m);
    expect(readme).toMatch(/off by default/i);
    expect(readme).toContain("Receipts in PR bodies");
    expect(readme).toContain("dcterminalv2://run/");
    expect(readme).toContain("scripts/release-mac.sh");
    expect(readme).toContain("--universal");
  });
});
