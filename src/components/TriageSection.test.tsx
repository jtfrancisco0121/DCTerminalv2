// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TriageApproval, TriageView } from "../clickup/api";

const view = (over: Partial<TriageView> = {}): TriageView => ({
  runDir: "/d/runs/r-tri",
  runId: "r-tri",
  listId: "901",
  digest: "d1",
  approval: null,
  posted: null,
  problem: null,
  proposal: {
    groups: [
      {
        n: 1,
        title: "Foundations",
        tickets: ["t1", "t2"],
        dependsOn: [],
        why: "first",
      },
      {
        n: 2,
        title: "Features",
        tickets: ["t3"],
        dependsOn: [1],
        why: "second",
      },
    ],
    needsYou: [{ ticket: "t4", reason: "which provider?" }],
    suggestions: [
      {
        index: 0,
        title: "Add a retry",
        why: "sync fails once",
        description: "Retry twice.",
      },
    ],
    closes: [{ ticket: "t5", why: "done in the code" }],
    outOfScope: [],
    names: { t1: "One", t2: "Two", t3: "Three", t4: "Four", t5: "Five" },
  },
  ...over,
});

let current: TriageView[] = [view()];
const triageOverview = vi.fn(async () => current);
const triageApprove = vi.fn(async (..._args: unknown[]): Promise<TriageApproval> => ({
  decision: "approved",
  creates: [],
  closes: [],
  groups: [],
  at: "2026-11-06T09:00:00Z",
  schema: 1,
  triageDigest: "d1",
}));
const triageApply = vi.fn(async (..._args: unknown[]) => []);
vi.mock("../clickup/api", () => ({
  triageOverview: () => triageOverview(),
  triageApprove: (...args: unknown[]) => triageApprove(...args),
  triageApply: (...args: unknown[]) => triageApply(...args),
}));

const { TriageSection, cardsOf, cardsProblem, editsOf, summaryLine } =
  await import("./TriageSection");

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  current = [view()];
});

describe("the triage edits", () => {
  it("renumbers dependencies when groups are reordered or rejected, and refuses a forward one", () => {
    const cards = cardsOf(view());
    expect(cardsProblem(cards)).toBeNull();
    // Group 2 first: it now depends on a group that comes after it.
    const swapped = [cards[1], cards[0]];
    expect(cardsProblem(swapped)).toContain("depends on group 2");
    // Rejecting the group it depends on removes the dependency.
    const rejected = [{ ...cards[0], keep: false }, cards[1]];
    expect(cardsProblem(rejected)).toBeNull();
    expect(editsOf(rejected, new Set(), new Set()).groups).toEqual([
      { title: "Features", tickets: ["t3"], dependsOn: [], why: "second" },
    ]);
    // Emptying a group is refused with its name.
    expect(cardsProblem([{ ...cards[1], tickets: [] }])).toContain(
      "would be empty",
    );
    expect(editsOf(cards, new Set([0]), new Set(["t5"]))).toMatchObject({
      creates: [0],
      closes: ["t5"],
    });
  });

  it("says what approval does in one line", () => {
    const cards = cardsOf(view());
    expect(summaryLine(cards, new Set(), new Set())).toBe(
      "Approving queues 2 groups, creates 0 tickets and closes 0 tickets in ClickUp.",
    );
    expect(summaryLine([cards[0]], new Set([0]), new Set(["t5"]))).toBe(
      "Approving queues 1 group, creates 1 ticket and closes 1 ticket in ClickUp.",
    );
  });
});

describe("TriageSection", () => {
  it("shows the groups, flags and unticked suggestions, and approves with exactly the ticked ones", async () => {
    render(<TriageSection />);
    await screen.findByTestId("triage-group-1");
    expect(screen.getByText(/which provider\?/)).toBeTruthy();
    expect(
      (screen.getByLabelText("Create ticket Add a retry") as HTMLInputElement)
        .checked,
    ).toBe(false);
    expect(
      (screen.getByLabelText("Close ticket t5") as HTMLInputElement).checked,
    ).toBe(false);
    fireEvent.click(screen.getByLabelText("Create ticket Add a retry"));
    expect(screen.getByTestId("triage-summary").textContent).toBe(
      "Approving queues 2 groups, creates 1 ticket and closes 0 tickets in ClickUp.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(triageApprove).toHaveBeenCalledOnce());
    const [runDir, digest, decision, edits] = triageApprove.mock.calls[0] as [
      string,
      string,
      string,
      { creates: number[]; closes: string[]; groups: unknown[] },
    ];
    expect([runDir, digest, decision]).toEqual([
      "/d/runs/r-tri",
      "d1",
      "approved",
    ]);
    expect(edits.creates).toEqual([0]);
    expect(edits.closes).toEqual([]);
    expect(edits.groups).toHaveLength(2);
  });

  it("refuses an edit that points a dependency forward, with the reason inline, and disables Approve", async () => {
    render(<TriageSection />);
    await screen.findByTestId("triage-group-1");
    fireEvent.click(
      screen.getByRole("button", { name: "Move Foundations down" }),
    );
    const refusal = await screen.findByTestId("triage-refusal");
    expect(refusal.textContent).toContain("depends on group");
    expect(
      (screen.getByRole("button", { name: "Approve" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    fireEvent.click(
      screen.getByRole("button", { name: "Move Foundations up" }),
    );
    expect(screen.queryByTestId("triage-refusal")).toBeNull();
  });

  it("moves a ticket to another group and can reject the whole proposal", async () => {
    render(<TriageSection />);
    await screen.findByTestId("triage-group-1");
    fireEvent.change(screen.getByLabelText("Move t3 to"), {
      target: { value: "1" },
    });
    expect(
      within(screen.getByTestId("triage-group-1")).getByText(/Three/),
    ).toBeTruthy();
    // Group 2 is now empty: refused.
    expect(screen.getByTestId("triage-refusal").textContent).toContain(
      "would be empty",
    );
    fireEvent.click(screen.getByRole("button", { name: "Reject" }));
    await waitFor(() => expect(triageApprove).toHaveBeenCalledOnce());
    expect(triageApprove.mock.calls[0][2]).toBe("rejected");
    expect(triageApply).not.toHaveBeenCalled();
  });

  it("applies the ClickUp writes right after an approval that ticked any", async () => {
    triageApprove.mockResolvedValueOnce({
      decision: "approved",
      creates: [{ index: 0, name: "n", description: "d" }],
      closes: [],
      groups: [],
      at: "t",
      schema: 1,
      triageDigest: "d1",
    });
    render(<TriageSection />);
    await screen.findByTestId("triage-group-1");
    fireEvent.click(screen.getByLabelText("Create ticket Add a retry"));
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() =>
      expect(triageApply).toHaveBeenCalledWith("/d/runs/r-tri"),
    );
  });

  it("shows an approved proposal read-only with each write's result and Retry for a failed one", async () => {
    current = [
      view({
        approval: {
          schema: 1,
          triageDigest: "d1",
          decision: "approved",
          at: "2026-11-06T09:00:00Z",
          closes: ["t5"],
          groups: [
            {
              n: 1,
              title: "Foundations",
              tickets: ["t1"],
              dependsOn: [],
              why: "",
            },
          ],
          creates: [{ index: 0, name: "Add a retry", description: "d" }],
        },
        posted: {
          items: {
            "create:0": {
              result: "failed",
              at: "t",
              message: "ClickUp refused it",
            },
            "close:t5": { result: "posted", at: "t" },
            "create:1": { result: "blocked", at: "t" },
          },
        },
      }),
    ];
    render(<TriageSection />);
    await screen.findByTestId("triage-approved-r-tri");
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    expect(screen.getByTestId("triage-write-close:t5").textContent).toContain(
      "sent",
    );
    expect(screen.getByTestId("triage-write-create:1").textContent).toContain(
      "looks like a secret",
    );
    fireEvent.click(
      within(screen.getByTestId("triage-write-create:0")).getByRole("button", {
        name: "Retry",
      }),
    );
    await waitFor(() =>
      expect(triageApply).toHaveBeenCalledWith("/d/runs/r-tri"),
    );
  });

  it("says a stale proposal changed and offers no Approve, and shows a rejection as one line", async () => {
    current = [
      view({
        problem: "The proposal changed. Review it again.",
        proposal: null,
      }),
    ];
    const { unmount } = render(<TriageSection />);
    expect(
      (await screen.findByTestId("triage-problem-r-tri")).textContent,
    ).toBe("The proposal changed. Review it again.");
    expect(screen.queryByRole("button", { name: "Approve" })).toBeNull();
    unmount();
    current = [
      view({
        approval: {
          schema: 1,
          triageDigest: "d1",
          decision: "rejected",
          at: "2026-11-06T09:00:00Z",
          closes: [],
          groups: [],
          creates: [],
        },
      }),
    ];
    render(<TriageSection />);
    expect(await screen.findByText(/Rejected on 2026-11-06/)).toBeTruthy();
  });
});
