// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ChangedTicket, DraftView, PostOutcome, PostReply, PostRequest, RunClickUp } from "./api";

const api = {
  state: vi.fn<(runDir: string) => Promise<RunClickUp>>(),
  check: vi.fn<(runDir: string) => Promise<ChangedTicket[]>>(),
  pull: vi.fn<(runDir: string, ids: string[]) => Promise<{ pulled: string[]; updateFile: string }>>(),
  post: vi.fn<(runDir: string, version: string, requests: PostRequest[]) => Promise<PostReply>>(),
  retry: vi.fn<(runDir: string, ticketId: string) => Promise<unknown>>(),
  consent: vi.fn<(runDir: string, sha: string) => Promise<void>>(),
  now: vi.fn<(runDir: string) => Promise<void>>(),
  merged: vi.fn<(runDir: string, ticketId: string, status: string) => Promise<PostOutcome>>(),
};
vi.mock("./api", () => ({
  clickupRunState: (d: string) => api.state(d),
  clickupCheckTickets: (d: string) => api.check(d),
  clickupPullTickets: (d: string, ids: string[]) => api.pull(d, ids),
  clickupPostWriteback: (d: string, v: string, r: PostRequest[]) => api.post(d, v, r),
  clickupAutoPostRetry: (d: string, id: string) => api.retry(d, id),
  clickupAutoPostConsent: (d: string, sha: string) => api.consent(d, sha),
  clickupAutoPostNow: (d: string) => api.now(d),
  clickupPostMergedStatus: (d: string, id: string, status: string) => api.merged(d, id, status),
}));

import { ClickUpRunSection, draftDone, tellClaude } from "./ClickUpRunSection";

const draft = (id: string, over: Partial<DraftView> = {}): DraftView => ({
  ticket: { id, customId: null, title: `Claude's words ${id}`, url: null },
  prs: [],
  comment: `Comment ${id}`,
  status: "in review",
  statusNote: null,
  posted: {},
  needsConfirm: false,
  trusted: { customId: `QA-${id}`, name: `Ticket ${id}`, url: `https://app.clickup.com/t/${id}`, list: "Sprint" },
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
  draftsVersion: "v1",
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

  it("lists drafts unticked with snapshot labels, and posts status and comment ticks apart", async () => {
    api.state.mockResolvedValue({
      ...base,
      drafts: [
        draft("86a"),
        draft("86b", { status: null, statusNote: "No PR covers this ticket." }),
        draft("86c", {
          posted: {
            comment: { result: "posted", at: "2026-10-23T11:00:00Z", value: "x" },
            status: { result: "posted", at: "t", value: "in review" },
          },
        }),
        draft("86d", { trusted: null }),
      ],
    });
    api.post.mockResolvedValue({
      outcomes: [{ ticketId: "86a", result: "posted", message: "comment posted" }],
      error: null,
    });
    render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    await waitFor(() => expect(screen.getByTestId("draft-86a")).toBeTruthy());
    const boxes = screen.getAllByRole("checkbox") as HTMLInputElement[];
    expect(boxes.every((b) => !b.checked)).toBe(true);
    // Labels from the snapshot, never writeback.json's title.
    expect(screen.getByText("Ticket 86a")).toBeTruthy();
    expect(screen.queryByText(/Claude's words/)).toBeNull();
    expect((screen.getByLabelText("Set status of QA-86c") as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByLabelText("Post comment on QA-86c") as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByTestId("draft-86d").textContent).toContain("not in this run's ticket snapshot");
    expect(within(screen.getByTestId("draft-86d")).queryByRole("checkbox")).toBeNull();
    expect(screen.getByText("Status unchanged: No PR covers this ticket.")).toBeTruthy();
    expect(screen.getByText(/Comment posted 2026-10-23 11:00/)).toBeTruthy();
    expect((screen.getByText(/Post ticked/) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText("Comment for QA-86a"), { target: { value: "Edited" } });
    fireEvent.click(screen.getByLabelText("Post comment on QA-86a"));
    fireEvent.click(screen.getByText("Post ticked (1)"));
    await waitFor(() => expect(screen.getByTestId("post-outcomes")).toBeTruthy());
    expect(api.post).toHaveBeenCalledWith("/r", "v1", [
      { ticketId: "86a", comment: "Edited", status: null, confirmResend: false, confirmSecret: false },
    ]);
  });

  it("sends the shown status when only the status is ticked", async () => {
    api.state.mockResolvedValue({ ...base, drafts: [draft("86a")] });
    api.post.mockResolvedValue({ outcomes: [], error: null });
    render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    await waitFor(() => expect(screen.getByTestId("draft-86a")).toBeTruthy());
    expect(screen.getByText(/list Sprint/)).toBeTruthy();
    fireEvent.click(screen.getByLabelText("Set status of QA-86a"));
    fireEvent.click(screen.getByText("Post ticked (1)"));
    await waitFor(() => expect(api.post).toHaveBeenCalled());
    expect(api.post.mock.calls[0][2]).toEqual([
      { ticketId: "86a", comment: null, status: "in review", confirmResend: false, confirmSecret: false },
    ]);
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
    fireEvent.click(screen.getByLabelText("Post comment on QA-86a"));
    fireEvent.click(screen.getByText(/I checked the ticket/));
    fireEvent.click(screen.getByText("Post ticked (1)"));
    await waitFor(() => expect(api.post).toHaveBeenCalled());
    expect(api.post.mock.calls[0][2][0].confirmResend).toBe(true);
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

describe("comments posted automatically", () => {
  it("lists posted, failed (with retry), blocked and skipped comments with ticket links", async () => {
    api.state.mockResolvedValue({
      ...base,
      drafts: [draft("86a1"), draft("86a2"), draft("86a3")],
      draftsVersion: "v1",
      autoPost: {
        enabled: true,
        running: false,
        problem: null,
        record: {
          schema: 1,
          writebackSha: "v1",
          at: "t",
          results: [
            { ticketId: "86a1", result: "posted", message: "comment posted", url: "https://app.clickup.com/t/86a1", at: "t", text: "Done: S1" },
            { ticketId: "86a2", result: "failed", message: "Token invalid", url: "https://app.clickup.com/t/86a2", at: "t" },
            { ticketId: "86a3", result: "blocked", message: "blocked: looks like a secret (a JWT).", url: null, at: "t" },
          ],
        },
      },
    });
    api.retry.mockResolvedValue({});
    render(<ClickUpRunSection runDir="/runs/r-1" stage="shipping" updatedAt="t" />);
    const list = await screen.findByTestId("autopost");
    expect(within(list).getByTestId("autopost-86a1").textContent).toContain("QA-86a1 posted");
    expect(within(list).getByTestId("autopost-86a1").textContent).toContain("open ticket");
    expect(within(list).getByTestId("autopost-86a1").querySelector("details pre")?.textContent).toBe("Done: S1");
    const failed = within(list).getByTestId("autopost-86a2");
    expect(failed.textContent).toContain("failed");
    expect(within(list).getByTestId("autopost-86a3").textContent).toContain("blocked: looks like a secret");
    expect(within(list).queryAllByText("Retry")).toHaveLength(1);
    fireEvent.click(within(failed).getByText("Retry"));
    await waitFor(() => expect(api.retry).toHaveBeenCalledWith("/runs/r-1", "86a2"));
    expect(list.textContent).toContain("Status changes are never posted on their own");
  });
});

describe("the click before comments post on their own", () => {
  const offer = {
    writebackSha: "sha-1",
    comments: [
      { ticketId: "86a1", label: "QA-86a1", text: "Done: S1" },
      { ticketId: "86a2", label: "QA-86a2", text: "Done: S2" },
      { ticketId: "86a3", label: "QA-86a3", text: "Done: S3" },
    ],
    notIncluded: [{ ticketId: "86a4", label: "QA-86a4", reason: "blocked: looks like a secret (a JWT)." }],
  };
  const view = (over: Partial<NonNullable<RunClickUp["autoPost"]>> = {}): RunClickUp => ({
    ...base,
    drafts: [draft("86a1"), draft("86a2"), draft("86a3"), draft("86a4")],
    autoPost: { enabled: true, running: false, problem: null, record: null, offer, ...over },
  });

  it("shows every comment collapsed, then one click sends the sha it showed", async () => {
    api.state.mockResolvedValue(view());
    api.consent.mockResolvedValue();
    render(<ClickUpRunSection runDir="/runs/r-1" stage="shipping" updatedAt="t" />);
    const block = await screen.findByRole("region", { name: "Comments ready to post" });
    expect(within(block).getByText("Comment for QA-86a1")).toBeTruthy();
    expect(block.querySelectorAll("details")).toHaveLength(3);
    expect(block.querySelector("details")?.hasAttribute("open")).toBe(false);
    expect(block.querySelector("details pre")?.textContent).toBe("Done: S1");
    const notIncluded = within(block).getByTestId("consent-not-included");
    expect(notIncluded.textContent).toContain("QA-86a4");
    expect(notIncluded.textContent).toContain("looks like a secret");
    expect(api.consent).not.toHaveBeenCalled();
    fireEvent.click(within(block).getByRole("button", { name: "Post 3 comments" }));
    await waitFor(() => expect(api.consent).toHaveBeenCalledWith("/runs/r-1", "sha-1"));
    await waitFor(() => expect(api.state.mock.calls.length).toBeGreaterThan(1));
  });

  it("offers no button when nothing can be posted on its own", async () => {
    api.state.mockResolvedValue(view({ offer: { ...offer, comments: [] } }));
    render(<ClickUpRunSection runDir="/r" stage="shipping" updatedAt="t" />);
    const block = await screen.findByRole("region", { name: "Comments ready to post" });
    expect(within(block).queryByRole("button")).toBeNull();
    expect(block.textContent).toContain("Nothing to include");
    expect(within(block).getByTestId("consent-not-included").textContent).toContain("QA-86a4");
  });

  it("says plainly when the click could not be recorded and leaves the button enabled", async () => {
    api.state.mockResolvedValue(view());
    api.consent.mockRejectedValue("The ship approval is not in status.json.");
    render(<ClickUpRunSection runDir="/r" stage="shipping" updatedAt="t" />);
    const button = await screen.findByRole("button", { name: "Post 3 comments" });
    fireEvent.click(button);
    await screen.findByText(/The click could not be recorded: The ship approval is not in status.json\. Nothing was posted\./);
    expect((screen.getByRole("button", { name: "Post 3 comments" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("shows no consent block once the click is recorded", async () => {
    api.state.mockResolvedValue(view({ offer: null, running: true }));
    render(<ClickUpRunSection runDir="/r" stage="shipping" updatedAt="t" />);
    await screen.findByText("Posting the comments…");
    expect(screen.queryByRole("region", { name: "Comments ready to post" })).toBeNull();
  });
});

describe("a comment that looks like it holds a secret", () => {
  it("warns under the comment, sends the confirm for that ticket only, and clears it after the post", async () => {
    api.state.mockResolvedValue({
      ...base,
      drafts: [
        draft("86a", { comment: "key ghp_abcdefghijklmnopqrstuvwxyz0123", secretKind: "a GitHub token" }),
        draft("86b"),
      ],
    });
    api.post.mockResolvedValue({
      outcomes: [
        { ticketId: "86a", result: "posted", message: "comment posted" },
        { ticketId: "86b", result: "posted", message: "comment posted" },
      ],
      error: null,
    });
    render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    const warning = await screen.findByText("This comment looks like it contains a GitHub token.");
    expect(warning.className).toContain("secret-warning");
    expect(screen.getByLabelText("Comment for QA-86a").getAttribute("aria-describedby")).toBe(warning.id);
    // Only the flagged ticket has the confirm box.
    const confirms = screen.getAllByLabelText("I checked this comment: post it anyway.") as HTMLInputElement[];
    expect(confirms).toHaveLength(1);
    expect(confirms[0].checked).toBe(false);
    expect(screen.queryByText("This comment looks like it contains", { exact: false })?.id).toBe("secret-86a");
    fireEvent.click(screen.getByLabelText("Post comment on QA-86a"));
    fireEvent.click(screen.getByLabelText("Post comment on QA-86b"));
    fireEvent.click(confirms[0]);
    fireEvent.click(screen.getByText("Post ticked (2)"));
    await waitFor(() => expect(api.post).toHaveBeenCalled());
    expect(api.post.mock.calls[0][2]).toEqual([
      { ticketId: "86a", comment: "key ghp_abcdefghijklmnopqrstuvwxyz0123", status: null, confirmResend: false, confirmSecret: true },
      { ticketId: "86b", comment: "Comment 86b", status: null, confirmResend: false, confirmSecret: false },
    ]);
    await waitFor(() => expect(screen.getByTestId("post-outcomes")).toBeTruthy());
    expect((screen.getByLabelText("I checked this comment: post it anyway.") as HTMLInputElement).checked).toBe(false);
  });

  it("shows the confirm after a secret refusal even when the text was edited, and no warning when it is clean", async () => {
    api.state.mockResolvedValue({ ...base, drafts: [draft("86a", { secretKind: "a Bearer token" }), draft("86b")] });
    api.post.mockResolvedValue({
      outcomes: [
        {
          ticketId: "86b",
          result: "not-sent",
          message: "Comment not sent: looks like it contains a webhook URL. Check the comment, tick the confirm box, then post again.",
        },
      ],
      error: null,
    });
    render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
    await screen.findByTestId("draft-86b");
    expect(within(screen.getByTestId("draft-86b")).queryByLabelText("I checked this comment: post it anyway.")).toBeNull();
    // Editing the flagged text away drops the warning (the post-time scan decides).
    fireEvent.change(screen.getByLabelText("Comment for QA-86a"), { target: { value: "clean now" } });
    expect(screen.queryByText(/looks like it contains a Bearer token/)).toBeNull();
    expect(within(screen.getByTestId("draft-86a")).queryByLabelText("I checked this comment: post it anyway.")).toBeNull();
    fireEvent.click(screen.getByLabelText("Post comment on QA-86b"));
    fireEvent.click(screen.getByText("Post ticked (1)"));
    await waitFor(() =>
      expect(within(screen.getByTestId("draft-86b")).getByLabelText("I checked this comment: post it anyway.")).toBeTruthy(),
    );
    expect(screen.getByTestId("post-outcomes").textContent).toContain("looks like it contains a webhook URL");
  });
});

describe("an automatic attempt that failed before anything was recorded", () => {
  const waitingView = (waiting: NonNullable<NonNullable<RunClickUp["autoPost"]>["waiting"]>): RunClickUp => ({
    ...base,
    drafts: [draft("86a1")],
    autoPost: { enabled: true, running: false, problem: null, record: null, offer: null, waiting },
  });

  it("says why, when the next try is, and Try now makes one attempt", async () => {
    api.state.mockResolvedValue(waitingView({ reason: "The posted record could not be saved: disk full.", nextAt: "2026-10-27T14:32:00Z", noToken: false }));
    api.now.mockResolvedValue();
    render(<ClickUpRunSection runDir="/runs/r-1" stage="shipping" updatedAt="t" />);
    const notice = await screen.findByRole("status");
    expect(notice.textContent).toContain("Not posted yet: The posted record could not be saved: disk full.");
    expect(notice.textContent).toMatch(/Next try at \d{2}:\d{2}\./);
    fireEvent.click(within(notice).getByText("Try now"));
    await waitFor(() => expect(api.now).toHaveBeenCalledWith("/runs/r-1"));
    await waitFor(() => expect(api.state.mock.calls.length).toBeGreaterThan(1));
  });

  it("with no token there is no time: it waits for Settings", async () => {
    api.state.mockResolvedValue(waitingView({ reason: "no ClickUp token for this account", nextAt: null, noToken: true }));
    render(<ClickUpRunSection runDir="/r" stage="shipping" updatedAt="t" />);
    const notice = await screen.findByTestId("backoff-notice");
    expect(notice.textContent).toContain(
      "Not posted yet: no ClickUp token for this account. Next try after you set one in Settings.",
    );
    expect(notice.textContent).not.toMatch(/\d{2}:\d{2}/);
  });

  it("hides the notice while an attempt runs", async () => {
    api.state.mockResolvedValue({
      ...waitingView({ reason: "x", nextAt: "2026-10-27T14:32:00Z", noToken: false }),
      autoPost: {
        enabled: true,
        running: true,
        problem: null,
        record: null,
        waiting: { reason: "x", nextAt: "2026-10-27T14:32:00Z", noToken: false },
      },
    });
    render(<ClickUpRunSection runDir="/r" stage="shipping" updatedAt="t" />);
    await screen.findByText("Posting the comments…");
    expect(screen.queryByTestId("backoff-notice")).toBeNull();
  });
});

describe("ClickUpRunSection merged status", () => {
  beforeEach(() => {
    Object.values(api).forEach((m) => m.mockReset());
  });

  describe("merged status row", () => {
    const withMerged = (merged: RunClickUp["merged"]): RunClickUp => ({
      ...base,
      drafts: [draft("86a", { status: null, comment: null })],
      merged,
    });

    it("is an unticked row that sends nothing until ticked, and then only the status", async () => {
      api.state.mockResolvedValue(withMerged([{ ticketId: "86a", status: "complete", note: null, posted: null }]));
      api.merged.mockResolvedValue({ ticketId: "86a", result: "posted", message: "status set to complete" });
      render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
      const row = await screen.findByTestId("merged-86a");
      const box = within(row).getByRole("checkbox") as HTMLInputElement;
      expect(box.checked).toBe(false);
      expect(row.textContent).toContain("Merged: set status to complete");
      expect(row.className).toContain("clickup-draft");
      const post = screen.getByRole("button", { name: /Post ticked/ }) as HTMLButtonElement;
      expect(post.disabled).toBe(true);
      expect(api.merged).not.toHaveBeenCalled();
      fireEvent.click(box);
      fireEvent.click(screen.getByRole("button", { name: "Post ticked (1)" }));
      await waitFor(() => expect(api.merged).toHaveBeenCalledWith("/r", "86a", "complete"));
      expect(api.post).not.toHaveBeenCalled();
      expect((await screen.findByTestId("post-outcomes")).textContent).toContain("status set to complete");
    });

    it("is a muted note without a checkbox when there is no done status or the read failed", async () => {
      api.state.mockResolvedValue(
        withMerged([
          { ticketId: "86a", status: null, note: "No done status in Sprint.", posted: null },
          { ticketId: "86b", status: null, note: "Could not read Sprint's statuses: offline", posted: null },
        ]),
      );
      render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
      const a = await screen.findByTestId("merged-86a");
      expect(a.textContent).toContain("No done status in Sprint.");
      expect(within(a).queryByRole("checkbox")).toBeNull();
      expect(within(screen.getByTestId("merged-86b")).queryByRole("checkbox")).toBeNull();
      expect(screen.getByTestId("merged-86b").textContent).toContain("Could not read");
    });

    it("shows a posted one as done, and nothing when no PR merged", async () => {
      api.state.mockResolvedValue(
        withMerged([
          {
            ticketId: "86a",
            status: "complete",
            note: null,
            posted: { result: "posted", at: "t", value: "complete" },
          },
        ]),
      );
      const { unmount } = render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
      const row = await screen.findByTestId("merged-86a");
      expect(row.textContent).toContain("status set to complete");
      expect(within(row).queryByRole("checkbox")).toBeNull();
      unmount();
      api.state.mockResolvedValue(withMerged([]));
      render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" />);
      await screen.findByText("Updates to post");
      expect(screen.queryByTestId("merged-86a")).toBeNull();
    });

    it("reloads and shows the row when the merge key changes, with stage and updatedAt unchanged", async () => {
      api.state.mockResolvedValueOnce(withMerged([]));
      const { rerender } = render(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" mergeKey="PR1:OPEN:" />);
      await screen.findByText("Updates to post");
      expect(screen.queryByTestId("merged-86a")).toBeNull();
      expect(api.state).toHaveBeenCalledTimes(1);
      api.state.mockResolvedValue(withMerged([{ ticketId: "86a", status: "complete", note: null, posted: null }]));
      rerender(<ClickUpRunSection runDir="/r" stage="done" updatedAt="1" mergeKey="PR1:MERGED:" />);
      expect(await screen.findByTestId("merged-86a")).toBeTruthy();
      expect(api.state).toHaveBeenCalledTimes(2);
    });
  });
});
