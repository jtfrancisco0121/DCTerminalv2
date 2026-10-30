// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Repo, RepoSetup } from "../run/api";
import { setRepoSetup } from "../run/api";
import { ensureWebStorage } from "../storage/webStorage";
import { parseCopyFiles, WorktreeSetup } from "./WorktreeSetup";
import { rememberedBmadMode, rememberedReviewOnGitHub } from "./StartScreen";
import { summaryText } from "./ImportDevDialog";

vi.mock("../run/api", async (original) => ({
  ...(await original<typeof import("../run/api")>()),
  setRepoSetup: vi.fn(async (_path: string, setup: RepoSetup) => setup),
}));

ensureWebStorage();

describe("start screen choices", () => {
  it("reviews on GitHub by default and remembers the last choice per window", () => {
    expect(rememberedReviewOnGitHub("win-9")).toBe(true);
    localStorage.setItem("dcv2.reviewOnGitHub.win-9", "off");
    expect(rememberedReviewOnGitHub("win-9")).toBe(false);
    expect(rememberedReviewOnGitHub("main")).toBe(true);
  });

  it("remembers the BMAD mode per start folder, Auto by default and for an unknown value", () => {
    expect(rememberedBmadMode("/w/a")).toBe("auto");
    localStorage.setItem("dcv2.bmadMode./w/a", "on");
    localStorage.setItem("dcv2.bmadMode./w/b", "banana");
    expect(rememberedBmadMode("/w/a")).toBe("on");
    expect(rememberedBmadMode("/w/b")).toBe("auto");
    expect(rememberedBmadMode("/w/c")).toBe("auto");
    localStorage.removeItem("dcv2.bmadMode./w/a");
    localStorage.removeItem("dcv2.bmadMode./w/b");
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

describe("worktree setup save", () => {
  const repo: Repo = { name: "mathlib", path: "/w/mathlib", check: null };
  const mocked = vi.mocked(setRepoSetup);
  afterEach(() => {
    cleanup();
    mocked.mockClear();
  });

  it("has no PR line limit field and saves no size limit", async () => {
    render(<WorktreeSetup repos={[repo]} />);
    expect(screen.queryByLabelText("PR line limit of mathlib")).toBeNull();
    expect(screen.queryByText(/PR line limit/)).toBeNull();
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() => expect(mocked).toHaveBeenCalledTimes(1));
    expect(Object.keys(mocked.mock.calls[0][1]).sort()).toEqual(["copyFiles", "liveTests", "setup"]);
  });
});
