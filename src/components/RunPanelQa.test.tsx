// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/run/run-view.json";
import type { PrView, QaView } from "../run/api";

const runQaImage = vi.fn(async (_runDir: string, name: string) => `data:image/png;base64,${name}`);

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("../run/api", async (original) => ({
  ...(await original<typeof import("../run/api")>()),
  runQaImage: (runDir: string, name: string) => runQaImage(runDir, name),
  readRun: vi.fn(async () => fixture),
  rerunChecks: vi.fn(async () => {}),
  prWorktreeChecks: vi.fn(async () => []),
}));

const { QaSection } = await import("./QaSection");
const { RunPanel } = await import("./RunPanel");

const pr = (state: string, qa: QaView | null): PrView => ({ ...(fixture.prs[0] as unknown as PrView), id: "PR1", state, qa });
const rows = (n: number, result = "PASS") =>
  Array.from({ length: n }, (_, i) => ({ id: `S1.${i + 1}`, result, text: `criterion ${i + 1}`, image: `S1.${i + 1}` }));
const qa = (result: string, tried: number, total: number, failed = 0, criteria = rows(total)): QaView => ({
  round: 1,
  result,
  tried,
  total,
  failed,
  path: "reviews/PR1-qa1.md",
  criteria,
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("the QA pill", () => {
  it.each([
    ["qa", null, "QA trying…"],
    ["reviewed", qa("pass", 2, 2), "QA 2/2 passed"],
    ["qa", qa("fail", 2, 2, 1), "QA failed"],
    ["needs-attention", qa("error", 0, 2, 0, []), "QA could not run"],
  ])("a PR in state %s shows %s", (state, result, text) => {
    render(<QaSection runDir="/d/runs/r-1" pr={pr(state, result)} />);
    expect(screen.getByTestId("qa-pill-PR1").textContent).toBe(text);
  });

  it("shows nothing for a PR with no QA result that is not being tried", () => {
    const { container } = render(<QaSection runDir="/d/runs/r-1" pr={pr("reviewed", null)} />);
    expect(container.innerHTML).toBe("");
  });

  it("a run panel PR without QA renders no QA block", async () => {
    render(<RunPanel runDir="/d/runs/r-1" />);
    await screen.findByTestId("pr-PR1");
    expect(screen.queryByTestId("qa-PR1")).toBeNull();
  });
});

describe("the criterion rows and thumbnails", () => {
  it("lists the criteria, draws six thumbnails and counts the rest", async () => {
    render(<QaSection runDir="/d/runs/r-1" pr={pr("qa", qa("fail", 8, 8, 1, [...rows(1, "FAIL"), ...rows(8).slice(1)]))} />);
    expect(screen.getAllByRole("listitem")).toHaveLength(8);
    expect(await screen.findAllByRole("button", { name: /^Screenshot of / })).toHaveLength(6);
    expect(screen.getByText("+2 more")).toBeTruthy();
    await waitFor(() => expect(runQaImage).toHaveBeenCalledTimes(6));
    expect(runQaImage).toHaveBeenCalledWith("/d/runs/r-1", "S1.1");
  });

  it("says image unavailable when the app refuses the file", async () => {
    runQaImage.mockRejectedValueOnce("not a PNG");
    render(<QaSection runDir="/d/runs/r-1" pr={pr("qa", qa("pass", 1, 1))} />);
    expect(await screen.findByText("image unavailable")).toBeTruthy();
  });
});

describe("the screenshot dialog", () => {
  it("opens on a thumbnail, steps with the arrow keys, closes with Escape and gives focus back", async () => {
    render(<QaSection runDir="/d/runs/r-1" pr={pr("reviewed", qa("pass", 2, 2))} />);
    const thumb = (await screen.findAllByRole("button", { name: /^Screenshot of / }))[0];
    thumb.focus();
    fireEvent.click(thumb);
    const dialog = await screen.findByRole("dialog");
    expect(dialog.textContent).toContain("S1.1 · PASS · 1 of 2");
    expect(await within(dialog).findByAltText("S1.1: criterion 1")).toBeTruthy();
    fireEvent.keyDown(screen.getByText("Next"), { key: "ArrowRight" });
    expect(dialog.textContent).toContain("S1.2 · PASS · 2 of 2");
    fireEvent.keyDown(screen.getByText("Next"), { key: "ArrowLeft" });
    expect(dialog.textContent).toContain("S1.1 · PASS · 1 of 2");
    fireEvent.keyDown(dialog, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(thumb);
  });
});
