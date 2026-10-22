import { describe, expect, it, vi } from "vitest";
import { firstOpenResumes, folderProblem } from "./openPlan";

const tab = (over: Partial<{ id: string; kind: "claude" | "shell"; hasSession: boolean }> = {}) => ({
  id: "tab_1",
  kind: "claude" as const,
  hasSession: true,
  ...over,
});

describe("the first open of a tab", () => {
  it("resumes a Claude tab's session when it can still be resumed", async () => {
    const lookup = vi.fn(async () => "685dd464-eabf-4e4c-9f6f-ac02c63aad27");
    await expect(firstOpenResumes(tab(), lookup)).resolves.toBe(true);
    expect(lookup).toHaveBeenCalledWith("tab_1");
  });

  it("starts fresh when the session is gone or the lookup fails", async () => {
    await expect(firstOpenResumes(tab(), async () => null)).resolves.toBe(false);
    await expect(firstOpenResumes(tab(), () => Promise.reject(new Error("no")))).resolves.toBe(false);
  });

  it("never asks for a new Claude tab or a shell", async () => {
    const lookup = vi.fn(async () => "id");
    await expect(firstOpenResumes(tab({ hasSession: false }), lookup)).resolves.toBe(false);
    await expect(firstOpenResumes(tab({ kind: "shell" }), lookup)).resolves.toBe(false);
    expect(lookup).not.toHaveBeenCalled();
  });

  it("does not start a tab whose folder is gone, and says why", () => {
    expect(folderProblem({ cwd: "/w/app", folderStatus: "ok" })).toBeNull();
    expect(folderProblem({ cwd: "/w/app", folderStatus: "missing" })).toContain("/w/app");
    expect(folderProblem({ cwd: "/w/a.txt", folderStatus: "not-a-directory" })).toContain("file");
    expect(folderProblem({ cwd: "", folderStatus: "empty" })).toBe("This tab has no folder.");
  });
});
