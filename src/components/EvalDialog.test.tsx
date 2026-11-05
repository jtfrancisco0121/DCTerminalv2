// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EvalPreview } from "../run/dispatch";

const dispatchEvalPreview = vi.fn();
const dispatchEvalStart = vi.fn();
const flightRunShare = vi.fn();
vi.mock("../run/dispatch", () => ({
  dispatchEvalPreview: (...args: unknown[]) => dispatchEvalPreview(...args),
  dispatchEvalStart: (...args: unknown[]) => dispatchEvalStart(...args),
}));
vi.mock("../flight/api", () => ({ flightRunShare: (...args: unknown[]) => flightRunShare(...args) }));

const { EvalDialog } = await import("./EvalDialog");

const NOW = 1_800_000_000;
const preview = (over: Partial<EvalPreview> = {}): EvalPreview => ({
  title: "Add multiply()",
  holdPercent: null,
  fiveHour: { percent: 30, resetsAt: NOW + 3600 },
  basedRepos: ["/w/app"],
  defaultBranchRepos: [],
  ...over,
});

const open = async (over: Partial<EvalPreview> = {}, onQueued = vi.fn(), onClose = vi.fn()) => {
  dispatchEvalPreview.mockResolvedValue(preview(over));
  render(<EvalDialog windowId="main" runDir="/d/runs/r-1" onClose={onClose} onQueued={onQueued} nowSec={() => NOW} />);
  await screen.findByText("Add multiply()");
  return { onQueued, onClose };
};
const start = () => screen.getByRole("button", { name: /^Start/ }) as HTMLButtonElement;

beforeEach(() => {
  dispatchEvalPreview.mockReset();
  dispatchEvalStart.mockReset();
  flightRunShare.mockReset();
  flightRunShare.mockResolvedValue(4);
});
afterEach(cleanup);

describe("EvalDialog", () => {
  it("shows N, the estimate before Start, the base line and the usage line; the estimate follows N", async () => {
    await open({ holdPercent: 80 });
    expect((screen.getByLabelText("Number of runs") as HTMLInputElement).value).toBe("3");
    expect(screen.getByTestId("eval-usage").textContent).toBe("Five-hour usage 30% · hold 80%");
    expect(screen.getByTestId("eval-base").textContent).toBe("Builds on the source run's commits");
    await waitFor(() => expect(screen.getByTestId("eval-estimate").textContent).toContain("about 12 points"));
    fireEvent.change(screen.getByLabelText("Number of runs"), { target: { value: "5" } });
    expect(screen.getByTestId("eval-estimate").textContent).toContain("about 20 points");
    expect(start().textContent).toBe("Start 5 runs");
    expect(start().disabled).toBe(false);
  });

  it("says 'not known' for the estimate when the source has no readings, and names a repo on the default branch", async () => {
    flightRunShare.mockResolvedValue(null);
    await open({ basedRepos: ["/w/app"], defaultBranchRepos: ["/w/docs"] });
    await waitFor(() => expect(screen.getByTestId("eval-estimate").textContent).toBe("Estimate: not known"));
    expect(screen.getByTestId("eval-base").textContent).toContain("current default branch for docs");
  });

  it("disables Start with the reason at or past the hold, and still shows the number", async () => {
    await open({ holdPercent: 80, fiveHour: { percent: 85, resetsAt: NOW + 600 } });
    expect(start().disabled).toBe(true);
    expect(screen.getByTestId("eval-reason").textContent).toBe("Five-hour usage is 85%, at or past your hold of 80%.");
    fireEvent.click(start());
    expect(dispatchEvalStart).not.toHaveBeenCalled();
  });

  it("refuses with 'usage not known' when a hold is set and there is no reading", async () => {
    await open({ holdPercent: 80, fiveHour: null });
    expect(start().disabled).toBe(true);
    expect(screen.getByTestId("eval-usage").textContent).toBe("Five-hour usage is not known");
    expect(screen.getByTestId("eval-reason").textContent).toContain("not known");
  });

  it("treats a reading past its reset as clear, and says so when no hold is set", async () => {
    await open({ holdPercent: 80, fiveHour: { percent: 95, resetsAt: NOW - 5 } });
    expect(start().disabled).toBe(false);
    expect(screen.queryByTestId("eval-reason")).toBeNull();
    cleanup();
    await open({ holdPercent: null, fiveHour: null });
    expect(screen.getByTestId("eval-usage").textContent).toBe("No usage hold is set");
    expect(start().disabled).toBe(false);
  });

  it("queues the runs and reports them, or shows the backend's refusal", async () => {
    dispatchEvalStart.mockResolvedValueOnce("b-9");
    const { onQueued } = await open();
    fireEvent.click(start());
    await waitFor(() => expect(onQueued).toHaveBeenCalledWith(3, "b-9"));
    expect(dispatchEvalStart).toHaveBeenCalledWith("main", "/d/runs/r-1", 3);
    cleanup();
    dispatchEvalStart.mockRejectedValueOnce("Not started: the five-hour usage is 90%.");
    const second = await open();
    fireEvent.click(start());
    expect((await screen.findByTestId("eval-error")).textContent).toContain("the five-hour usage is 90%");
    expect(second.onQueued).not.toHaveBeenCalled();
  });

  it("allows 2 to 10 runs only", async () => {
    await open();
    for (const bad of ["1", "11", ""]) {
      fireEvent.change(screen.getByLabelText("Number of runs"), { target: { value: bad } });
      expect(start().disabled).toBe(true);
      expect(screen.getByTestId("eval-range").textContent).toBe("Pick between 2 and 10 runs.");
    }
    fireEvent.change(screen.getByLabelText("Number of runs"), { target: { value: "10" } });
    expect(start().disabled).toBe(false);
  });
});
