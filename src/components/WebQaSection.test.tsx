// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SettingsView } from "../bridge";
import type { PassView } from "../run/api";

const row = (charter: string, title: string, result: string, comparison: PassView["rows"][number]["comparison"]) => ({
  charter,
  title,
  result,
  reason: `${title} reason`,
  shot: `webqa/shots/${charter}.png`,
  comparison,
});
const finding = (title: string) => ({
  key: title.toLowerCase(),
  title,
  severity: "high",
  steps: `steps of ${title}`,
  expected: "a button",
  seen: "nothing",
  shot: "webqa/shots/B.png",
  comparison: "new" as const,
});
const pass = (over: Partial<PassView> = {}): PassView => ({
  passId: "r-20261109-101500-a1b2",
  runDir: "/runs/r-20261109-101500-a1b2",
  release: false,
  createdAt: "2026-11-09T10:15:00Z",
  stage: "done",
  targetId: "t-1",
  targetName: "Staging",
  running: false,
  result: "fail",
  problem: null,
  rows: [row("A", "Sign in", "PASS", "fixed"), row("B", "Search", "FAIL", "new")],
  findings: [finding("Search returns nothing"), finding("Login is slow")],
  comparison: { first: false, fixedFindings: [] },
  clickupListId: "901",
  posted: {},
  ...over,
});

let passes: PassView[] = [];
const overview = vi.fn(async () => passes);
const approve = vi.fn(async (_pass: string, _ticked: string[]) => ({}));
const apply = vi.fn(async (_window: string | null, _pass: string) => [{ item: "create:0", result: "posted", message: "created" }]);
const image = vi.fn(async (_dir: string, name: string) => (name.endsWith("B") ? Promise.reject(new Error("gone")) : `data:image/png;base64,${name}`));
const release = vi.fn();
const resume = vi.fn(async (_window: string, _dir: string) => ({ tab: {}, runDir: "/x" }));
let settings: Partial<SettingsView> = { webQa: { targets: [{ id: "t-1", name: "Staging", allowlist: ["https://a/"], username: "u", clickupListId: "901", charters: [] }] } };
vi.mock("../bridge", () => ({ getSettings: async () => settings }));
vi.mock("../run/api", () => ({
  webqaOverview: () => overview(),
  webqaApprove: (p: string, t: string[]) => approve(p, t),
  webqaApply: (w: string | null, p: string) => apply(w, p),
  webqaReleaseOpen: (v: string) => release(v),
  webqaStart: vi.fn(),
  runQaImage: (d: string, n: string) => image(d, n),
  resumeRun: (w: string, d: string) => resume(w, d),
}));

const { WebQaSection } = await import("./WebQaSection");

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  passes = [];
  settings = { webQa: { targets: [{ id: "t-1", name: "Staging", allowlist: ["https://a/"], username: "u", clickupListId: "901", charters: [] }] } };
});

describe("WebQaSection", () => {
  it("shows the empty state and a Start button that is off, with the reason, when there is no target", async () => {
    settings = { webQa: { targets: [] } };
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    expect(await screen.findByText("No web QA passes yet.")).toBeTruthy();
    const start = screen.getByRole("button", { name: "Start web QA pass…" }) as HTMLButtonElement;
    await waitFor(() => expect(start.disabled).toBe(true));
    expect(start.title).toBe("Add a target in Settings › Web QA first.");
  });

  it("shows no hint when a target exists", async () => {
    passes = [pass()];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    expect(await screen.findByText("Staging")).toBeTruthy();
    expect(screen.queryByText("No targets now; earlier passes are kept.")).toBeNull();
    expect((screen.getByRole("button", { name: "Start web QA pass…" }) as HTMLButtonElement).title).toBe("");
  });

  it("with no target and earlier passes, says so once above the passes, which stay listed", async () => {
    settings = { webQa: { targets: [] } };
    passes = [pass()];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    const hint = await screen.findByText("No targets now; earlier passes are kept.");
    expect(screen.getAllByText("No targets now; earlier passes are kept.")).toHaveLength(1);
    const list = screen.getByRole("list");
    expect(hint.compareDocumentPosition(list) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(within(list).getByText("Staging")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show results" })).toBeTruthy();
  });

  it("shows result rows with comparison badges, the screenshot, and its absence", async () => {
    passes = [pass()];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show results" }));
    expect(screen.getByText("Sign in reason")).toBeTruthy();
    const table = screen.getByRole("table");
    expect(within(table).getByText("Fixed")).toBeTruthy();
    expect(within(table).getByText("New")).toBeTruthy();
    expect(await within(table).findByText("Screenshot missing")).toBeTruthy();
    expect(await within(table).findByAltText("Screenshot of the charter")).toBeTruthy();
  });

  it("hides the badges and says so on a first pass", async () => {
    passes = [pass({ comparison: { first: true, fixedFindings: [] }, findings: [] })];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show results" }));
    expect(screen.getByText("First pass: nothing to compare yet.")).toBeTruthy();
    expect(screen.queryByText("Fixed")).toBeNull();
    expect(screen.queryByText("New")).toBeNull();
  });

  it("shows a running pass with a link to its tab, and a refused result with the reason", async () => {
    passes = [
      pass({ passId: "r-20261110-000000-aaaa", running: true, stage: "planning", result: null, rows: [], findings: [] }),
      pass({ passId: "r-20261108-000000-bbbb", result: null, rows: [], findings: [], problem: "Pass results are not valid: the results changed after they were checked." }),
    ];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    expect(await screen.findByRole("status")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Go to tab" }));
    await waitFor(() => expect(resume).toHaveBeenCalledWith("main", "/runs/r-20261109-101500-a1b2"));
    expect(screen.getByText(/Pass results are not valid: the results changed/)).toBeTruthy();
  });

  it("asks before creating tickets, names the count and the list, and sends only the ticked findings", async () => {
    passes = [pass()];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show results" }));
    const create = screen.getByRole("button", { name: "Create tickets" }) as HTMLButtonElement;
    expect(create.disabled).toBe(true);
    expect(create.title).toBe("Tick at least one finding.");
    fireEvent.click(screen.getByRole("checkbox", { name: "Search returns nothing" }));
    expect(create.disabled).toBe(false);
    fireEvent.click(create);
    expect(screen.getByText("Create 1 ClickUp task in 901?")).toBeTruthy();
    expect(approve).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(apply).toHaveBeenCalledWith("main", "r-20261109-101500-a1b2"));
    expect(approve).toHaveBeenCalledWith("r-20261109-101500-a1b2", ["search returns nothing"]);
  });

  it("disables Create tickets, with the reason, when the target has no ClickUp list", async () => {
    passes = [pass({ clickupListId: null })];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show results" }));
    fireEvent.click(screen.getByRole("checkbox", { name: "Login is slow" }));
    const create = screen.getByRole("button", { name: "Create tickets" }) as HTMLButtonElement;
    expect(create.disabled).toBe(true);
    expect(create.title).toBe("Add a ClickUp list to this target in Settings › Web QA.");
  });

  it("marks a finding that was sent as Created and cannot be ticked again", async () => {
    passes = [pass({ posted: { "create:0": { result: "posted", at: "t", message: "created" } } })];
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Show results" }));
    expect(screen.getByText("Created")).toBeTruthy();
    expect((screen.getByRole("checkbox", { name: "Search returns nothing" }) as HTMLInputElement).disabled).toBe(true);
  });

  it("opens release QA results by version", async () => {
    release.mockResolvedValueOnce(pass({ passId: "release-0.10.0", release: true, runDir: null, targetName: "Release 0.10.0", targetId: null, result: "pass", rows: [row("first-launch", "First launch", "PASS", null)], findings: [] }));
    render(<WebQaSection windowId="main" onStarted={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "Open release QA results…" }));
    fireEvent.change(screen.getByLabelText("Release version"), { target: { value: "0.10.0" } });
    fireEvent.click(screen.getByRole("button", { name: "Open" }));
    expect(await screen.findByText("Release 0.10.0")).toBeTruthy();
    expect(release).toHaveBeenCalledWith("0.10.0");
  });
});
