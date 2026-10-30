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
  it("has no Size column", () => {
    card();
    expect(screen.queryByRole("columnheader", { name: "Size" })).toBeNull();
    expect(screen.queryByTestId("approval-size-PR1")).toBeNull();
    expect(screen.queryByTestId("approval-over-PR2")).toBeNull();
    expect(within(screen.getByTestId("approval-PR1")).getByText("PR1")).toBeTruthy();
  });
});

describe("ApprovalCard Epic column", () => {
  it("has an Epic column and orders rows by epic group, then plan order", () => {
    const epics = [["PR1", "e1"], ["PR2", "e2"], ["PR3", "e1"]];
    const prs = view.prs.map((pr) => {
      const hit = epics.find(([id]) => id === pr.id);
      return hit ? { ...pr, epic: { id: hit[1], title: hit[1] === "e1" ? "Billing" : null } } : pr;
    });
    card(prs);
    expect(screen.getByRole("columnheader", { name: "Epic" })).toBeTruthy();
    expect(screen.getByTestId("approval-epic-PR1").textContent).toBe("Billing");
    expect(screen.getByTestId("approval-epic-PR2").textContent).toBe("e2");
    expect(screen.getByTestId("approval-epic-PR4").textContent).toBe("–");
    expect(screen.getByTestId("approval-epic-PR1").getAttribute("title")).toBe("Billing (e1)");
    expect(screen.getByTestId("approval-epic-PR2").getAttribute("title")).toBe("e2");
    expect(screen.getByTestId("approval-epic-PR4").getAttribute("title")).toBeNull();
    const order = screen.getAllByTestId(/^approval-PR\d+$/).map((row) => row.getAttribute("data-testid"));
    expect(order.slice(0, 3)).toEqual(["approval-PR1", "approval-PR3", "approval-PR2"]);
    expect(order[order.length - 1]).toBe("approval-PR5");
  });

  it("keeps plan order and a dash for an old run", () => {
    card();
    expect(screen.getByTestId("approval-epic-PR1").textContent).toBe("–");
    expect(screen.getAllByTestId(/^approval-PR\d+$/).map((r) => r.getAttribute("data-testid"))).toEqual(
      view.prs.map((pr) => `approval-${pr.id}`),
    );
  });
});
