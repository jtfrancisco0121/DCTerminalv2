// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChangedTicket, DraftView, PostReply, PostRequest, RunClickUp } from "./api";

const api = {
  state: vi.fn<(runDir: string) => Promise<RunClickUp>>(),
  check: vi.fn<(runDir: string) => Promise<ChangedTicket[]>>(),
  pull: vi.fn<(runDir: string, ids: string[]) => Promise<{ pulled: string[]; updateFile: string }>>(),
  post: vi.fn<(runDir: string, requests: PostRequest[]) => Promise<PostReply>>(),
};
vi.mock("./api", () => ({
  clickupRunState: (d: string) => api.state(d),
  clickupCheckTickets: (d: string) => api.check(d),
  clickupPullTickets: (d: string, ids: string[]) => api.pull(d, ids),
  clickupPostWriteback: (d: string, r: PostRequest[]) => api.post(d, r),
}));

import { ClickUpRunSection, draftDone, tellClaude } from "./ClickUpRunSection";

const draft = (id: string, over: Partial<DraftView> = {}): DraftView => ({
  ticket: { id, customId: null, title: `Ticket ${id}`, url: null },
  prs: [],
  comment: `Comment ${id}`,
  status: "in review",
  statusNote: null,
  posted: {},
  needsConfirm: false,
  inTickets: true,
  ...over,
});

const base: RunClickUp = {
  hasTickets: true,
  ticketCount: 2,
  fetchedAt: "2026-10-23T10:00:00Z",
  ticketsProblem: null,
  pulled: [],
  updateFile: null,
  drafts: null,
  writebackProblem: null,
};

describe("ClickUpRunSection", () => {
  beforeEach(() => {
    Object.values(api).forEach((m) => m.mockReset());
  });

  it("checks once at plan approval and pulls a changed ticket in, telling the user what to say", async () => {
    api.state.mockResolvedValue(base);
    api.check.mockResolvedValue([
      { id: "86a", customId: "QA-1", title: "Export", missing: false, dateUpdated: "x", changes: ["status: open → blocked"] },
      { id: "86b", customId: null, title: "Gone", missing: true, dateUpdated: null, changes: ["ClickUp no longer has it"] },
    ]);
    api.pull.mockResolvedValue({ pulled: ["86a"], updateFile: "/runs/r-1/tickets-update.md" });
    const { rerender } = render(<ClickUpRunSection runDir="/runs/r-1" stage="plan-approval" updatedAt="1" />);
    await waitFor(() => expect(screen.getByTestId("changed-86a")).toBeTruthy());
    expect(api.check).toHaveBeenCalledTimes(1);
    expect(screen.getByText("ticket changed")).toBeTruthy();
    expect(screen.getByText("ticket gone")).toBeTruthy();
    expect(within(screen.getByTestId("changed-86b")).queryByText("Pull it in")).toBeNull();
    rerender(<ClickUpRunSection runDir="/runs/r-1" stage="plan-approval" updatedAt="2" />);
    fireEvent.click(screen.getByText("Pull it in"));
    await waitFor(() => expect(screen.getByTestId("pull-note")).toBeTruthy());
    expect(api.pull).toHaveBeenCalledWith("/runs/r-1", ["86a"]);
    expect(screen.getByTestId("pull-note").textContent).toContain(tellClaude(["86a"], "/runs/r-1/tickets-update.md"));
    expect(screen.queryByTestId("changed-86a")).toBeNull();
    expect(api.check).toHaveBeenCalledTimes(1);
  });

  it("does not check on its own before plan approval", async () => {
    api.state.mockResolvedValue(base);
    render(<ClickUpRunSection runDir="/r" stage="planning" updatedAt={null} />);
    await waitFor(() => expect(screen.getByText("Check for ticket changes")).toBeTruthy());
    expect(api.check).not.toHaveBeenCalled();
  });

  it("lists drafts unticked, posts only ticked ones with the edited comment", async () => {
    api.state.mockResolvedValue({
      ...base,
      drafts: [
        draft("86a"),
        draft("86b", { status: null, statusNote: "No PR covers this ticket." }),
        draft("86c", { posted: { comment: { result: "posted", at: "2026-10-23T11:00:00Z", value: "x" }, status: { result: "posted", at: "t", value: "in review" } } }),
        draft("86d", { inTickets: false }),
      ],
    });
    api.post.mockResolvedValue({ outcomes: [{ ticketId: "86a", result: "posted", message: "status set to in review, comment posted" }], error: null });
    render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    await waitFor(() => expect(screen.getByTestId("draft-86a")).toBeTruthy());
    const boxes = screen.getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.every((b) => !b.checked)).toBe(true);
    expect((screen.getByLabelText("Post 86c") as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByLabelText("Post 86d") as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByText("Status unchanged: No PR covers this ticket.")).toBeTruthy();
    expect(screen.getByText(/Comment posted 2026-10-23 11:00/)).toBeTruthy();
    expect((screen.getByText(/Post ticked/) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Comment for 86a"), { target: { value: "Edited" } });
    fireEvent.click(screen.getByLabelText("Post 86a"));
    fireEvent.click(screen.getByText("Post ticked (1)"));
    await waitFor(() => expect(screen.getByTestId("post-outcomes")).toBeTruthy());
    expect(api.post).toHaveBeenCalledWith("/r", [{ ticketId: "86a", comment: "Edited", status: true, confirmResend: false }]);
  });

  it("asks for a confirmation before resending a comment that may have posted", async () => {
    api.state.mockResolvedValue({
      ...base,
      drafts: [draft("86a", { needsConfirm: true, posted: { comment: { result: "maybe", at: "t", value: "c" } } })],
    });
    api.post.mockResolvedValue({ outcomes: [], error: null });
    render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    await waitFor(() => expect(screen.getByText(/may already have this comment/)).toBeTruthy());
    expect(screen.getByText(/Comment may have been posted/)).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Post 86a"));
    fireEvent.click(screen.getByText(/I checked the ticket/));
    fireEvent.click(screen.getByText("Post ticked (1)"));
    await waitFor(() => expect(api.post).toHaveBeenCalled());
    expect(api.post.mock.calls[0][1][0].confirmResend).toBe(true);
  });

  it("shows nothing for a run without tickets or drafts", async () => {
    api.state.mockResolvedValue({ ...base, hasTickets: false });
    const { container } = render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    await waitFor(() => expect(api.state).toHaveBeenCalled());
    expect(container.textContent).toBe("");
  });

  it("knows when a draft has nothing left to send", () => {
    expect(draftDone(draft("a"))).toBe(false);
    expect(draftDone(draft("a", { comment: null, status: null }))).toBe(true);
    expect(draftDone(draft("a", { status: null, posted: { comment: { result: "posted", at: "", value: "" } } }))).toBe(true);
  });
});
