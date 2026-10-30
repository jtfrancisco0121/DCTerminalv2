// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SlopReport } from "../run/api";
import { SlopCheck } from "./RunPanel";

afterEach(cleanup);

const report = (over: Partial<SlopReport>): SlopReport => ({ state: "ok", findings: [], total: 0, detail: null, ...over });

describe("Slop check results", () => {
  it("lists findings grouped by file and copies file:line on a click", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.assign(navigator, { clipboard: { writeText } });
    const onSlopCheck = vi.fn(async () =>
      report({
        total: 3,
        findings: [
          { path: "src/a.ts", line: 3, rule: "console-log", message: "console.log left" },
          { path: "src/a.ts", line: 9, rule: "console-log", message: "console.log left" },
          { path: "lib.rs", line: 2, rule: "dbg-macro", message: "dbg left" },
        ],
      }),
    );
    render(<SlopCheck prId="PR1" onSlopCheck={onSlopCheck} />);
    fireEvent.click(screen.getByText("Slop check"));
    const results = await screen.findByTestId("slop-results-PR1");
    expect(onSlopCheck).toHaveBeenCalledWith("PR1");
    expect(results.querySelectorAll(".run-slop-file")).toHaveLength(2);
    expect(results.textContent).toContain("3 findings");
    fireEvent.click(screen.getAllByText("line 9")[0]!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("src/a.ts:9"));
  });

  it("says so when nothing was found", async () => {
    render(<SlopCheck prId="PR1" onSlopCheck={async () => report({})} />);
    fireEvent.click(screen.getByText("Slop check"));
    expect(await screen.findByTestId("slop-none-PR1")).toBeTruthy();
  });

  it("shows the worktree-missing state", async () => {
    render(<SlopCheck prId="PR2" onSlopCheck={async () => report({ state: "worktree-missing", detail: "/x/wt is gone" })} />);
    fireEvent.click(screen.getByText("Slop check"));
    const note = await screen.findByTestId("slop-missing-PR2");
    expect(note.textContent).toContain("worktree is gone");
  });

  it("shows an error from the backend", async () => {
    render(<SlopCheck prId="PR1" onSlopCheck={async () => Promise.reject("not where this run's worktrees go")} />);
    fireEvent.click(screen.getByText("Slop check"));
    expect(await screen.findByText(/not where this run/)).toBeTruthy();
  });
});
