// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { ensureWebStorage } from "../storage/webStorage";
import { parseCopyFiles } from "./WorktreeSetup";
import { rememberedReviewOnGitHub } from "./StartScreen";
import { summaryText } from "./ImportDevDialog";

ensureWebStorage();

describe("start screen choices", () => {
  it("reviews on GitHub by default and remembers the last choice per window", () => {
    expect(rememberedReviewOnGitHub("win-9")).toBe(true);
    localStorage.setItem("dcv2.reviewOnGitHub.win-9", "off");
    expect(rememberedReviewOnGitHub("win-9")).toBe(false);
    expect(rememberedReviewOnGitHub("main")).toBe(true);
  });

  it("reads the files to copy one per line or comma-separated", () => {
    expect(parseCopyFiles(" .env\n\n.env.local, cfg/dev.json ")).toEqual([".env", ".env.local", "cfg/dev.json"]);
    expect(parseCopyFiles("")).toEqual([]);
  });

  it("says what the dev import brought in", () => {
    expect(
      summaryText({ windows: ["win-2"], tabs: 1, closedTabs: 2, runs: 1, accountsAdded: ["Work"], memoryCopied: true, notes: [] }),
    ).toBe("Imported 1 tab in 1 window, 2 recently closed, 1 run adopted in place, memory copied, accounts added: Work. Reopen the windows from File › Reopen Closed Window.");
  });
});
