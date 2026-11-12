// @vitest-environment jsdom
// The start screen's request source (Describe it / From ClickUp) and its one Options line: only the visible source is
// ever sent, counted or checked, and the other keeps what was typed or fetched.
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ClickUpStart } from "../clickup/api";
import type { SplitChoice } from "../clickup/groups";

const startRun = vi.hoisted(() => vi.fn());
const dispatchOverview = vi.hoisted(() => vi.fn());
const dispatchEnqueue = vi.hoisted(() => vi.fn());
const dispatchRuns = vi.hoisted(() => vi.fn());
vi.mock("../run/api", async () => ({
  duplicateOf: () => null,
  runOverview: vi.fn(async () => ({ pluginDir: "/p", pluginVersion: null, roles: [], runs: [] })),
  detectFolderRepos: vi.fn(async () => ({ kind: "repo", repos: [{ name: "api", path: "/w/api", check: null }] })),
  startRun,
  resumeRun: vi.fn(),
  readRunFile: vi.fn(),
}));
vi.mock("../run/dispatch", () => ({ dispatchOverview, dispatchEnqueue, dispatchRuns }));
vi.mock("../bridge", () => ({
  recentFolders: vi.fn(async () => []),
  getSettings: vi.fn(async () => ({ run: { maxImplementers: 3, stopHooks: true } })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn(), openUrl: vi.fn() }));
// An uncontrolled field: what is typed in it survives only while the component stays mounted.
vi.mock("./WorktreeSetup", () => ({ WorktreeSetup: () => <input aria-label="Setup command" data-testid="worktree-setup" /> }));
vi.mock("./MemoryPanel", () => ({ MemoryPanel: () => null }));
const TICKETS: ClickUpStart = { refs: [], ticketIds: ["T-1", "T-2"] };
vi.mock("../clickup/ClickUpIntake", () => ({
  ClickUpIntake: ({
    onChange,
    onTooMany,
    onSplit,
  }: {
    onChange: (start: ClickUpStart | null) => void;
    onTooMany: (text: string | null) => void;
    onSplit: (choice: SplitChoice) => void;
  }) => (
    <div data-testid="intake">
      <button type="button" onClick={() => onChange(TICKETS)}>
        fetch tickets
      </button>
      <button type="button" onClick={() => onSplit({ perTicket: true, letters: {} })}>
        one run per ticket
      </button>
      <button type="button" onClick={() => onTooMany("150 tickets, remove some")}>
        too many
      </button>
    </div>
  ),
}));

const { StartScreen } = await import("./StartScreen");

beforeEach(() => {
  for (const mock of [startRun, dispatchOverview, dispatchEnqueue, dispatchRuns]) mock.mockReset();
  startRun.mockResolvedValue({ tab: {}, runDir: "/d/runs/new" });
  dispatchEnqueue.mockResolvedValue("q-1");
  dispatchRuns.mockResolvedValue(undefined);
  dispatchOverview.mockResolvedValue({ atCap: false, cap: 2, queue: [], usageHoldPercent: null, holders: [] });
});
afterEach(cleanup);

const open = async () => {
  render(<StartScreen windowId="main" initialFolder="/w/api" onStarted={() => {}} onPlainTab={() => {}} />);
  await screen.findByRole("radiogroup");
  await screen.findByTestId("repos");
};
const describeIt = () => screen.getByRole("radio", { name: "Describe it" });
const fromClickUp = () => screen.getByRole("radio", { name: "From ClickUp" });
const type = (text: string) => fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: text } });
const hiddenIn = (el: Element) => el.closest("[hidden]") !== null;
const startButton = () => screen.getByRole("button", { name: /^Start Eagle-Eye/ }) as HTMLButtonElement;

describe("the Options line", () => {
  it("is collapsed on open with the summary of the defaults, and expands on click", async () => {
    await open();
    const toggle = screen.getByRole("button", { name: /^Options/ });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.textContent).toBe("Options3 implementers · GitHub review · BMAD auto");
    expect(hiddenIn(screen.getByLabelText("Implementers"))).toBe(true);
    expect(hiddenIn(screen.getByLabelText("On"))).toBe(true);
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(hiddenIn(screen.getByLabelText("Implementers"))).toBe(false);
    // The folder, the request and Start never sit under it.
    expect(hiddenIn(startButton())).toBe(false);
    expect(hiddenIn(screen.getByRole("button", { name: "Plain tab" }))).toBe(false);
  });

  it("always shows the current values", async () => {
    await open();
    const summary = () => document.querySelector(".start-options-summary")!.textContent;
    fireEvent.click(screen.getByRole("button", { name: /^Options/ }));
    fireEvent.change(screen.getByLabelText("Implementers"), { target: { value: "5" } });
    expect(summary()).toBe("5 implementers · GitHub review · BMAD auto");
    fireEvent.click(screen.getByRole("checkbox", { name: /Review on GitHub/ }));
    expect(summary()).toBe("5 implementers · Local review · BMAD auto");
    fireEvent.click(screen.getByLabelText("On"));
    expect(summary()).toBe("5 implementers · Local review · BMAD on");
    // Epic waves show once tickets are visible and the box is ticked.
    fireEvent.click(fromClickUp());
    fireEvent.click(screen.getByText("fetch tickets"));
    fireEvent.click(screen.getByRole("checkbox", { name: /Plan an epic/ }));
    expect(summary()).toBe("5 implementers · Local review · Epic waves · BMAD on");
    fireEvent.click(describeIt());
    expect(summary()).toBe("5 implementers · Local review · BMAD on");
  });

  it("keeps a Worktree setup edit when collapsed", async () => {
    await open();
    const toggle = screen.getByRole("button", { name: /^Options/ });
    fireEvent.click(toggle);
    fireEvent.change(screen.getByTestId("worktree-setup"), { target: { value: "npm ci" } });
    fireEvent.click(toggle);
    expect(hiddenIn(screen.getByTestId("worktree-setup"))).toBe(true);
    fireEvent.click(toggle);
    expect((screen.getByTestId("worktree-setup") as HTMLInputElement).value).toBe("npm ci");
  });
});

describe("the request source", () => {
  it("shows the textarea by default, the intake after choosing From ClickUp, and keeps typed text", async () => {
    await open();
    expect(describeIt().getAttribute("aria-checked")).toBe("true");
    expect(hiddenIn(screen.getByPlaceholderText(/Describe the feature/))).toBe(false);
    expect(hiddenIn(screen.getByTestId("intake"))).toBe(true);
    type("Add multiply()");
    fireEvent.click(fromClickUp());
    expect(fromClickUp().getAttribute("aria-checked")).toBe("true");
    expect(hiddenIn(screen.getByPlaceholderText(/Describe the feature/))).toBe(true);
    expect(hiddenIn(screen.getByTestId("intake"))).toBe(false);
    fireEvent.click(describeIt());
    expect((screen.getByPlaceholderText(/Describe the feature/) as HTMLTextAreaElement).value).toBe("Add multiply()");
  });

  it("moves with the arrow keys", async () => {
    await open();
    describeIt().focus();
    fireEvent.keyDown(describeIt(), { key: "ArrowRight" });
    expect(fromClickUp().getAttribute("aria-checked")).toBe("true");
    expect(document.activeElement).toBe(fromClickUp());
    fireEvent.keyDown(fromClickUp(), { key: "ArrowLeft" });
    expect(describeIt().getAttribute("aria-checked")).toBe("true");
  });

  it("does not send hidden typed text in From ClickUp mode", async () => {
    await open();
    type("hidden words");
    fireEvent.click(fromClickUp());
    fireEvent.click(screen.getByText("fetch tickets"));
    fireEvent.click(startButton());
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    const args = startRun.mock.calls[0];
    expect(args[2]).toBe("Do the ClickUp tickets below.");
    expect(args[4]).toEqual(TICKETS);
    expect(JSON.stringify(args)).not.toContain("hidden words");
  });

  it("does not send fetched tickets in Describe mode: one run, no dispatch, plain label", async () => {
    await open();
    fireEvent.click(fromClickUp());
    fireEvent.click(screen.getByText("fetch tickets"));
    fireEvent.click(screen.getByText("one run per ticket"));
    expect(startButton().textContent).toBe("Start Eagle-Eye (2 runs)");
    fireEvent.click(describeIt());
    type("Add multiply()");
    expect(startButton().textContent).toBe("Start Eagle-Eye");
    fireEvent.click(startButton());
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    expect(startRun.mock.calls[0][2]).toBe("Add multiply()");
    expect(startRun.mock.calls[0][4]).toBeNull();
    expect(dispatchRuns).not.toHaveBeenCalled();
    // Switching back loses nothing.
    fireEvent.click(fromClickUp());
    expect(startButton().textContent).toBe("Start Eagle-Eye (2 runs)");
  });

  it("splits into one run per ticket in From ClickUp mode", async () => {
    await open();
    fireEvent.click(fromClickUp());
    fireEvent.click(screen.getByText("fetch tickets"));
    fireEvent.click(screen.getByText("one run per ticket"));
    fireEvent.click(startButton());
    await vi.waitFor(() => expect(dispatchRuns).toHaveBeenCalledOnce());
    expect(dispatchRuns.mock.calls[0][1]).toHaveLength(2);
    expect(startRun).not.toHaveBeenCalled();
  });

  it("queues a Describe-mode start with no tickets when the run limit is reached", async () => {
    dispatchOverview.mockResolvedValue({ atCap: true, cap: 1, queue: [], usageHoldPercent: null, holders: [] });
    await open();
    fireEvent.click(fromClickUp());
    fireEvent.click(screen.getByText("fetch tickets"));
    fireEvent.click(describeIt());
    type("Add multiply()");
    fireEvent.click(startButton());
    fireEvent.click(await screen.findByRole("button", { name: "Queue it" }));
    await vi.waitFor(() => expect(dispatchEnqueue).toHaveBeenCalledOnce());
    expect(dispatchEnqueue.mock.calls[0][0]).toMatchObject({ request: "Add multiply()", clickup: null, runMode: null });
  });

  it("does not let a too-many ticket set block Describe mode, only From ClickUp", async () => {
    await open();
    fireEvent.click(fromClickUp());
    fireEvent.click(screen.getByText("fetch tickets"));
    fireEvent.click(screen.getByText("too many"));
    expect(screen.getByTestId("start-reason").textContent).toBe("150 tickets, remove some");
    expect(startButton().disabled).toBe(true);
    fireEvent.click(describeIt());
    type("Add multiply()");
    expect(startButton().disabled).toBe(false);
    expect(screen.queryByTestId("start-reason")).toBeNull();
  });

  it("asks for a request when only the hidden source has content", async () => {
    await open();
    type("typed");
    fireEvent.click(fromClickUp());
    expect(startButton().disabled).toBe(true);
    expect(screen.getByTestId("start-reason").textContent).toBe("Say what should be done, or fetch ClickUp tickets.");
  });
});
