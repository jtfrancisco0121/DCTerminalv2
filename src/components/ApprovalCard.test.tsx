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
