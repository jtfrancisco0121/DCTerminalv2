// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Stepper } from "./Stepper";

const none = { done: 0, total: 0 };

describe("Stepper", () => {
  afterEach(cleanup);

  it("is a list of run steps with one current step", () => {
    render(<Stepper stage="building" waitingFor={null} prs={{ done: 2, total: 3 }} />);
    const list = screen.getByRole("list", { name: "Run steps" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(6);
    const current = list.querySelectorAll('[aria-current="step"]');
    expect(current).toHaveLength(1);
    expect(current[0].textContent).toContain("Building");
    expect(current[0].textContent).toContain("2/3 PRs");
  });

  it("rings the step waited at and says so", () => {
    render(<Stepper stage="ship-approval" waitingFor="ship-approval" prs={none} />);
    const item = screen.getByTitle(/Ship approval \(your step\): waiting for you/);
    expect(item.className).toContain("step-waiting");
    expect(item.className).toContain("step-you");
    expect(item.querySelector(".step-dot")?.className).toContain("tone-you");
    expect(item.getAttribute("aria-current")).toBe("step");
  });

  it("names both approval steps as the user's even when not waiting", () => {
    render(<Stepper stage="building" waitingFor={null} prs={none} />);
    expect(screen.getByTitle("Plan approval (your step)").textContent).toContain("(you)");
    expect(screen.getByTitle("Ship approval (your step)").textContent).toContain("(you)");
    expect(screen.getByTitle("Planning").textContent).not.toContain("(you)");
  });

  it("the vertical form shows dots with the names in the title", () => {
    render(<Stepper stage="building" waitingFor={null} prs={none} variant="vertical" />);
    const list = screen.getByRole("list", { name: "Run steps" });
    expect(list.className).toContain("stepper-vertical");
    expect(list.textContent).toBe("");
    for (const title of ["Planning", "Plan approval (your step)", "Building", "Ship approval (your step)", "Shipping", "Done"]) {
      expect(screen.getByTitle(title)).toBeTruthy();
    }
  });

  it("the compact form names only the current step", () => {
    render(<Stepper stage="building" waitingFor={null} prs={none} variant="compact" />);
    expect(screen.getByRole("list").textContent).toBe("Building");
  });

  it("a failed run ends in a danger Failed marker after the steps reached", () => {
    render(<Stepper stage="failed" waitingFor={null} prs={none} reached="building" />);
    const failed = screen.getByText("Failed").closest("li")!;
    expect(failed.querySelector(".step-dot")?.className).toContain("tone-danger");
    expect(screen.getByTitle("Building").className).toContain("step-done");
    expect(screen.getByTitle("Shipping").className).toContain("step-todo");
  });

  it("an epic-plan run has three steps", () => {
    render(<Stepper stage="planning" waitingFor={null} prs={none} mode="epic-plan" />);
    expect(screen.getAllByRole("listitem")).toHaveLength(3);
  });

  it("renders nothing for an unknown stage or none", () => {
    const { container } = render(<Stepper stage="time-travel" waitingFor={null} prs={none} />);
    expect(container.firstChild).toBeNull();
    cleanup();
    expect(render(<Stepper stage={null} waitingFor={null} prs={none} />).container.firstChild).toBeNull();
  });
});
