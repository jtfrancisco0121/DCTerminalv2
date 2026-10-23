// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IntakePreview } from "./api";

const fetchMock = vi.fn<(windowId: string, refs: unknown[]) => Promise<IntakePreview>>();
vi.mock("./api", () => ({ clickupFetch: (w: string, r: unknown[]) => fetchMock(w, r) }));

import { ClickUpIntake } from "./ClickUpIntake";

const preview: IntakePreview = {
  tickets: [
    { id: "86b1aa001", customId: "QA-129", title: "Sync stalls", status: "open", list: "QA", url: "u1" },
    { id: "86b1aa002", customId: null, title: "Duplicate lines", status: "in progress", list: "QA", url: "u2" },
  ],
  excluded: [{ id: "86b1aa003", customId: null, title: "Old", status: "closed", list: "QA", url: "u3", reason: "Status is \"closed\" (closed)" }],
  problems: [{ line: 2, message: "ClickUp has no task with that id, or this account can't see it." }],
  tooMany: null,
};

describe("ClickUpIntake", () => {
  beforeEach(() => {
    fetchMock.mockReset();
  });

  it("fetches pasted links with the window's account, lists tickets and removes one", async () => {
    fetchMock.mockResolvedValue(preview);
    const onChange = vi.fn();
    render(<ClickUpIntake windowId="win-1" onChange={onChange} />);
    fireEvent.change(screen.getByPlaceholderText(/Paste ClickUp/), {
      target: { value: "https://app.clickup.com/t/86b1aa001\n86b1aa009" },
    });
    fireEvent.click(screen.getByText("Fetch"));
    await waitFor(() => expect(screen.getByTestId("ticket-86b1aa001")).toBeTruthy());
    expect(fetchMock.mock.calls[0][0]).toBe("win-1");
    expect(fetchMock.mock.calls[0][1]).toHaveLength(2);
    expect(screen.getByText(/Line 2:/)).toBeTruthy();
    expect(screen.getByText(/Left out: 86b1aa003/)).toBeTruthy();
    expect(screen.getByTestId("ticket-86b1aa002").textContent).toContain("in progress · QA");
    expect(onChange).toHaveBeenLastCalledWith(
      expect.objectContaining({ ticketIds: ["86b1aa001", "86b1aa002"] }),
    );
    fireEvent.click(screen.getByLabelText("Remove QA-129"));
    expect(screen.queryByTestId("ticket-86b1aa001")).toBeNull();
    expect(onChange).toHaveBeenLastCalledWith(expect.objectContaining({ ticketIds: ["86b1aa002"] }));
    fireEvent.click(screen.getByLabelText("Remove 86b1aa002"));
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it("says why a fetch failed and sends nothing for text that is no link", async () => {
    fetchMock.mockImplementation(() => Promise.reject(new Error("No ClickUp token yet. Add one under Settings › ClickUp.")));
    render(<ClickUpIntake windowId="w" onChange={vi.fn()} />);
    const box = screen.getByPlaceholderText(/Paste ClickUp/);
    fireEvent.change(box, { target: { value: "hello there" } });
    expect(screen.getByText(/Line 1 is not a ClickUp/)).toBeTruthy();
    fireEvent.click(screen.getByText("Fetch"));
    expect(fetchMock).not.toHaveBeenCalled();
    fireEvent.change(box, { target: { value: "86b1aa001" } });
    fireEvent.click(screen.getByText("Fetch"));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("No ClickUp token yet"));
  });
});
