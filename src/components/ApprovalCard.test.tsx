// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/run/run-view.json";
import type { RunView } from "../run/api";
import { ApprovalCard } from "./ApprovalCard";

const view = fixture as unknown as RunView;

function card(prs = view.prs) {
  render(<ApprovalCard view={{ ...view, prs }} waitingFor="ship-approval" onReadPlan={vi.fn()} onGoToPrompt={vi.fn()} />);
}

afterEach(cleanup);

describe("ApprovalCard at the ship approval", () => {
  it("has a Size column: the lines, or a dash without a recorded size", () => {
    card();
    expect(screen.getByRole("columnheader", { name: "Size" })).toBeTruthy();
    expect(screen.getByTestId("approval-size-PR1").textContent).toBe("312");
    expect(screen.getByTestId("approval-size-PR3").textContent).toBe("–");
    expect(screen.queryByTestId("approval-over-PR1")).toBeNull();
  });

  it("shows an over-limit PR's limit and approved reason in its row", () => {
    card();
    const row = within(screen.getByTestId("approval-PR2"));
    const over = row.getByTestId("approval-over-PR2");
    expect(over.textContent).toBe("over 400: the parser and its table must ship together");
    expect(screen.getByTestId("approval-size-PR2").textContent).toContain("512");
  });

  it("says no reason, and cuts a long reason to 200 characters with all of it in the title", () => {
    const long = "x".repeat(300);
    const prs = view.prs.map((pr) => {
      if (pr.id === "PR1") return { ...pr, size: { lines: 500, files: 2, excluded: 0, limit: 400, over: true, reason: null } };
      if (pr.id === "PR2") return { ...pr, size: { lines: 500, files: 2, excluded: 0, limit: 400, over: true, reason: long } };
      return pr;
    });
    card(prs);
    expect(screen.getByTestId("approval-over-PR1").textContent).toBe("over 400: no reason");
    const cut = screen.getByTestId("approval-over-PR2");
    expect(cut.textContent!.length).toBe(201);
    expect(cut.textContent!.endsWith("…")).toBe(true);
    expect(cut.getAttribute("title")).toBe(`over 400: ${long}`);
  });
});
