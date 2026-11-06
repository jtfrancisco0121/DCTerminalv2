// @vitest-environment jsdom
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TriageScheduleRow } from "../bridge";
import { runSteps } from "../run/words";

const triageStart = vi.fn(async (..._args: unknown[]) => "q-1");
const triageBlocking = vi.fn(
  async (_list: string): Promise<string | null> => null,
);
vi.mock("../clickup/api", () => ({
  triageStart: (...args: unknown[]) => triageStart(...args),
  triageBlocking: (list: string) => triageBlocking(list),
}));
vi.mock("../bridge", () => ({ recentFolders: vi.fn(async () => ["/w/app"]) }));

const { TriageStartDialog, listIdOf } = await import("./TriageStartDialog");
const { TriageSchedule } = await import("./SettingsPage");

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  triageBlocking.mockResolvedValue(null);
});

describe("the triage start dialog", () => {
  it("reads a list id from a link or a bare id", () => {
    expect(listIdOf("901800112233")).toBe("901800112233");
    expect(
      listIdOf("https://app.clickup.com/9012345678/v/li/901800112233"),
    ).toBe("901800112233");
    expect(listIdOf("not a list")).toBeNull();
    expect(listIdOf("")).toBeNull();
  });

  it("disables Start with the reason until a list is named, then starts with the list and folder", async () => {
    const onStarted = vi.fn();
    const onClose = vi.fn();
    render(
      <TriageStartDialog
        windowId="main"
        onClose={onClose}
        onStarted={onStarted}
      />,
    );
    const start = screen.getByRole("button", {
      name: "Start triage",
    }) as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    expect(screen.getByTestId("triage-start-reason").textContent).toContain(
      "list link or its id",
    );
    fireEvent.change(screen.getByLabelText("ClickUp list"), {
      target: { value: "901800112233" },
    });
    await waitFor(() =>
      expect(
        (screen.getByLabelText("Triage folder") as HTMLInputElement).value,
      ).toBe("/w/app"),
    );
    await waitFor(() => expect(start.disabled).toBe(false));
    fireEvent.click(start);
    await waitFor(() =>
      expect(triageStart).toHaveBeenCalledWith(
        "main",
        "901800112233",
        "/w/app",
      ),
    );
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(onStarted).toHaveBeenCalled();
  });

  it("names the blocking run and keeps Start disabled", async () => {
    triageBlocking.mockResolvedValue(
      "the triage run r-1 of this list is not finished yet",
    );
    render(
      <TriageStartDialog
        windowId="main"
        onClose={() => {}}
        onStarted={() => {}}
      />,
    );
    fireEvent.change(screen.getByLabelText("ClickUp list"), {
      target: { value: "901800112233" },
    });
    await waitFor(() =>
      expect(screen.getByTestId("triage-start-reason").textContent).toContain(
        "r-1",
      ),
    );
    expect(
      (
        screen.getByRole("button", {
          name: "Start triage",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
  });

  it("shows a refusal from the app inline", async () => {
    triageStart.mockRejectedValueOnce(
      "No triage started: a triage of this list is already queued.",
    );
    render(
      <TriageStartDialog
        windowId="main"
        onClose={() => {}}
        onStarted={() => {}}
      />,
    );
    fireEvent.change(screen.getByLabelText("ClickUp list"), {
      target: { value: "901800112233" },
    });
    await waitFor(() =>
      expect(
        (
          screen.getByRole("button", {
            name: "Start triage",
          }) as HTMLButtonElement
        ).disabled,
      ).toBe(false),
    );
    fireEvent.click(screen.getByRole("button", { name: "Start triage" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "already queued",
    );
  });
});

describe("the triage schedule rows", () => {
  it("adds, edits and removes a row", () => {
    const rows: TriageScheduleRow[] = [
      {
        accountId: "a",
        listId: "901",
        time: "08:00",
        on: true,
        folder: "/w/app",
      },
    ];
    const onChange = vi.fn();
    render(
      <TriageSchedule
        rows={rows}
        accounts={[{ id: "a", name: "Personal" }]}
        onChange={onChange}
      />,
    );
    fireEvent.change(screen.getByLabelText("Triage schedule row 1 time"), {
      target: { value: "09:30" },
    });
    expect(onChange).toHaveBeenLastCalledWith([{ ...rows[0], time: "09:30" }]);
    fireEvent.click(screen.getByLabelText("Triage schedule row 1 on"));
    expect(onChange).toHaveBeenLastCalledWith([{ ...rows[0], on: false }]);
    fireEvent.click(
      screen.getByRole("button", { name: "Add a triage schedule row" }),
    );
    expect(onChange).toHaveBeenLastCalledWith([
      rows[0],
      { accountId: "a", listId: "", time: "08:00", on: false, folder: "" },
    ]);
    fireEvent.click(
      screen.getByRole("button", { name: "Remove triage schedule row 1" }),
    );
    expect(onChange).toHaveBeenLastCalledWith([]);
  });
});

describe("the stepper of a triage run", () => {
  it("has two steps (planning, done) and no approval stops", () => {
    const steps = runSteps(
      "planning",
      null,
      { done: 0, total: 0 },
      { mode: "triage" },
    );
    expect(steps?.steps.map((s) => s.key)).toEqual(["planning", "done"]);
    expect(steps?.steps.every((s) => !s.you)).toBe(true);
    expect(
      runSteps(
        "done",
        null,
        { done: 0, total: 0 },
        { mode: "triage" },
      )?.steps.every((s) => s.status === "done"),
    ).toBe(true);
  });
});
