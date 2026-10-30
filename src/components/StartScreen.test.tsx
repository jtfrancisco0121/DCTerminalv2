// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RunListItem } from "../run/api";

const day = (n: number) => new Date(Date.now() - n * 86_400_000).toISOString();
const run = (n: number, over: Partial<RunListItem> = {}): RunListItem => ({
  runId: `r-${n}`,
  runDir: `/d/runs/r-${n}`,
  folder: "/w/app",
  createdAt: day(n),
  stage: "done",
  updatedAt: null,
  title: `Run ${n}`,
  hasReport: true,
  ...over,
});
// Newest first: 10 runs, the 2nd unfinished.
const runs = Array.from({ length: 10 }, (_, n) => run(n + 1, n === 1 ? { stage: "building", hasReport: false } : {}));

const readRunFile = vi.fn(async (_dir: string, _path: string) => "# the report");
const revealItemInDir = vi.fn(async (_path: string) => {});
vi.mock("../run/api", async () => ({
  duplicateOf: (await vi.importActual<typeof import("../run/api")>("../run/api")).duplicateOf,
  runOverview: vi.fn(async () => ({ pluginDir: "/p", pluginVersion: null, roles: [], runs })),
  detectFolderRepos: vi.fn(async () => ({ kind: "none", repos: [] })),
  startRun: vi.fn(),
  resumeRun: vi.fn(),
  readRunFile: (dir: string, path: string) => readRunFile(dir, path),
}));
vi.mock("../bridge", () => ({
  recentFolders: vi.fn(async () => ["/w/app", "/w/web"]),
  getSettings: vi.fn(async () => ({ run: { maxImplementers: 3, stopHooks: true } })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (path: string) => revealItemInDir(path),
  openUrl: vi.fn(),
}));
vi.mock("./WorktreeSetup", () => ({ WorktreeSetup: () => null }));
vi.mock("./MemoryPanel", () => ({
  MemoryPanel: ({ onRefreshStale }: { onRefreshStale?: (request: string) => void }) => (
    <button type="button" onClick={() => onRefreshStale?.("stale backend")}>
      mock refresh stale
    </button>
  ),
}));
vi.mock("../clickup/ClickUpIntake", () => ({ ClickUpIntake: () => null }));

const { StartScreen, disabledReason } = await import("./StartScreen");

describe("StartScreen earlier runs", () => {
  afterEach(cleanup);

  it("lists the 8 newest with a relative date, and Show all opens the Runs view", async () => {
    const onShowAllRuns = vi.fn();
    render(
      <StartScreen windowId="main" initialFolder="" onStarted={() => {}} onPlainTab={() => {}} onShowAllRuns={onShowAllRuns} />,
    );
    const list = await screen.findByRole("region", { name: "Earlier runs" });
    expect(within(list).getAllByRole("listitem")).toHaveLength(8);
    expect(within(list).getByText("Run 1").parentElement!.textContent).toContain("yesterday");
    expect(within(list).getByText("Run 4").parentElement!.textContent).toContain("4 days ago");
    expect(within(list).queryByText("Run 9")).toBeNull();
    fireEvent.click(within(list).getByRole("button", { name: "Show all (10)" }));
    expect(onShowAllRuns).toHaveBeenCalledOnce();
  });

  it("offers Report and Open folder on done runs and Resume on the others", async () => {
    render(
      <StartScreen windowId="main" initialFolder="" onStarted={() => {}} onPlainTab={() => {}} onShowAllRuns={() => {}} />,
    );
    const list = await screen.findByRole("region", { name: "Earlier runs" });
    const done = within(within(list).getByText("Run 1").closest("li")!);
    const open = within(within(list).getByText("Run 2").closest("li")!);
    expect(done.getByText("Report")).toBeTruthy();
    expect(done.queryByText("Resume")).toBeNull();
    expect(open.getByText("Resume")).toBeTruthy();
    expect(open.queryByText("Report")).toBeNull();
    expect(open.queryByText("Open folder")).toBeNull();
    fireEvent.click(done.getByText("Open folder"));
    expect(revealItemInDir).toHaveBeenCalledWith("/d/runs/r-1");
    fireEvent.click(done.getByText("Report"));
    expect(await screen.findByText("# the report")).toBeTruthy();
    expect(readRunFile).toHaveBeenCalledWith("/d/runs/r-1", "report.md");
  });
});

describe("StartScreen duplicate start", () => {
  afterEach(cleanup);

  it("asks before starting the same request again; Resume resumes, Start anyway forces", async () => {
    const api = await import("../run/api");
    const startRun = api.startRun as unknown as ReturnType<typeof vi.fn>;
    const resumeRun = api.resumeRun as unknown as ReturnType<typeof vi.fn>;
    const detect = api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>;
    detect.mockResolvedValue({ kind: "repo", repos: [{ name: "app", path: "/w/app", check: null }] });
    startRun.mockRejectedValueOnce({ duplicate: { runId: "r-2", runDir: "/d/runs/r-2" } });
    startRun.mockResolvedValue({ tab: {}, runDir: "/d/runs/new" });
    resumeRun.mockResolvedValue({ tab: {}, runDir: "/d/runs/r-2", focused: true });
    const onStarted = vi.fn();
    render(<StartScreen windowId="main" initialFolder="/w/app" onStarted={onStarted} onPlainTab={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Add multiply()" } });
    const start = await screen.findByRole("button", { name: "Start Eagle-Eye" });
    await vi.waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(start);
    expect(await screen.findByText("This run is already in progress")).toBeTruthy();
    expect(startRun.mock.calls[0][7]).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Resume that run" }));
    await vi.waitFor(() => expect(resumeRun).toHaveBeenCalledWith("main", "/d/runs/r-2"));
    expect(onStarted).toHaveBeenCalledOnce();

    startRun.mockRejectedValueOnce({ duplicate: { runId: "r-2", runDir: "/d/runs/r-2" } });
    await vi.waitFor(() =>
      expect((screen.getByRole("button", { name: "Start Eagle-Eye" }) as HTMLButtonElement).disabled).toBe(false),
    );
    fireEvent.click(screen.getByRole("button", { name: "Start Eagle-Eye" }));
    fireEvent.click(await screen.findByRole("button", { name: "Start anyway" }));
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledTimes(3));
    expect(startRun.mock.calls[2][7]).toBe(true);
    expect(onStarted).toHaveBeenCalledTimes(2);
  });
});

describe("StartScreen near-limit warning", () => {
  afterEach(cleanup);

  const five = (utilization: number) => ({
    rateLimitType: "five_hour",
    label: "5h",
    utilization,
    resetsAt: Date.now() / 1000 + 3600,
    status: "allowed",
    seenAtMs: Date.now(),
  });

  it("warns at 80% on the 5-hour window without disabling Start", async () => {
    const api = await import("../run/api");
    (api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      kind: "repo",
      repos: [{ name: "app", path: "/w/app", check: null }],
    });
    render(
      <StartScreen windowId="main" initialFolder="/w/app" onStarted={() => {}} onPlainTab={() => {}} usageWindows={[five(83)]} />,
    );
    expect((await screen.findByTestId("near-limit")).textContent).toContain("5-hour limit is at 83%");
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Add multiply()" } });
    const start = screen.getByRole("button", { name: "Start Eagle-Eye" }) as HTMLButtonElement;
    await vi.waitFor(() => expect(start.disabled).toBe(false));
  });

  it("shows nothing below 80%", async () => {
    render(
      <StartScreen windowId="main" initialFolder="" onStarted={() => {}} onPlainTab={() => {}} usageWindows={[five(60)]} />,
    );
    await screen.findByRole("button", { name: "Start Eagle-Eye" });
    expect(screen.queryByTestId("near-limit")).toBeNull();
  });
});

describe("StartScreen recent folders", () => {
  afterEach(cleanup);

  it("offers the recent folders as suggestions for the folder field", async () => {
    const { container } = render(
      <StartScreen windowId="main" initialFolder="" onStarted={() => {}} onPlainTab={() => {}} />,
    );
    const input = screen.getByPlaceholderText("/path/to/repo or folder of repos");
    const list = input.getAttribute("list")!;
    await vi.waitFor(() => expect(container.querySelectorAll(`datalist#${list} option`)).toHaveLength(2));
  });
});

describe("StartScreen disabled Start", () => {
  afterEach(cleanup);

  const ready = {
    folder: "/w/app",
    found: { kind: "repo", repos: [{ name: "app", path: "/w/app", check: null }] } as never,
    hasRequest: true,
    hasTickets: false,
    tooMany: null,
    busy: false,
  };

  it("gives the first matching reason, in order", () => {
    expect(disabledReason({ ...ready, folder: " " })).toBe("Choose a folder first.");
    expect(disabledReason({ ...ready, found: { kind: "none", repos: [] } })).toBe(
      "This folder has no git repo. Pick a repo or a folder of repos.",
    );
    expect(disabledReason({ ...ready, found: { kind: "inside-repo", repos: [] } })).toBe("Pick the repo's top folder.");
    expect(disabledReason({ ...ready, hasRequest: false })).toBe("Say what should be done, or fetch ClickUp tickets.");
    expect(disabledReason({ ...ready, hasRequest: false, hasTickets: true })).toBeNull();
    expect(disabledReason({ ...ready, noRepoTicked: true })).toBe("Tick at least one repo.");
    expect(disabledReason({ ...ready, tooMany: "150 tickets, remove some" })).toBe("150 tickets, remove some");
    expect(disabledReason({ ...ready, busy: true })).toBe("Starting…");
    expect(disabledReason(ready)).toBeNull();
  });

  it("shows the reason on screen and as the button's description, none when enabled", async () => {
    const api = await import("../run/api");
    (api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      kind: "repo",
      repos: [{ name: "app", path: "/w/app", check: null }],
    });
    render(<StartScreen windowId="main" initialFolder="/w/app" onStarted={() => {}} onPlainTab={() => {}} />);
    const start = screen.getByRole("button", { name: "Start Eagle-Eye" }) as HTMLButtonElement;
    await vi.waitFor(() =>
      expect(screen.getByTestId("start-reason").textContent).toBe("Say what should be done, or fetch ClickUp tickets."),
    );
    expect(start.disabled).toBe(true);
    expect(start.getAttribute("aria-describedby")).toBe("start-reason");
    expect(document.getElementById("start-reason")?.textContent).toContain("Say what should be done");
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Add multiply()" } });
    expect(start.disabled).toBe(false);
    expect(screen.queryByTestId("start-reason")).toBeNull();
    expect(start.getAttribute("aria-describedby")).toBeNull();
  });
});

describe("StartScreen repos and implementers", () => {
  afterEach(cleanup);

  const workspace = {
    kind: "workspace",
    repos: [
      { name: "api", path: "/w/api", check: null },
      { name: "web", path: "/w/web", check: null },
    ],
  };

  it("offers tick boxes only for a folder of several repos; none ticked disables Start", async () => {
    const api = await import("../run/api");
    const detect = api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>;
    detect.mockResolvedValue({ kind: "repo", repos: [{ name: "app", path: "/w/app", check: null }] });
    const { unmount } = render(<StartScreen windowId="main" initialFolder="/w/app" onStarted={() => {}} onPlainTab={() => {}} />);
    await screen.findByLabelText("Implementers");
    expect(screen.queryByRole("checkbox", { name: "api" })).toBeNull();
    unmount();

    detect.mockResolvedValue(workspace);
    render(<StartScreen windowId="main" initialFolder="/w" onStarted={() => {}} onPlainTab={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Do it" } });
    const api1 = (await screen.findByRole("checkbox", { name: "api" })) as HTMLInputElement;
    const web = screen.getByRole("checkbox", { name: "web" }) as HTMLInputElement;
    expect(api1.checked && web.checked).toBe(true);
    fireEvent.click(api1);
    fireEvent.click(web);
    expect(screen.getByTestId("start-reason").textContent).toBe("Tick at least one repo.");
    expect((screen.getByRole("button", { name: "Start Eagle-Eye" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("passes the ticked repos and the implementer count, clamped on blur", async () => {
    const api = await import("../run/api");
    const detect = api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>;
    const startRun = api.startRun as unknown as ReturnType<typeof vi.fn>;
    startRun.mockReset();
    startRun.mockResolvedValue({ tab: {}, runDir: "/d/runs/new" });
    detect.mockResolvedValue(workspace);
    render(<StartScreen windowId="main" initialFolder="/w" onStarted={() => {}} onPlainTab={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Do it" } });
    fireEvent.click(await screen.findByRole("checkbox", { name: "web" }));
    const field = screen.getByLabelText("Implementers") as HTMLInputElement;
    expect(field.value).toBe("3");
    fireEvent.change(field, { target: { value: "12" } });
    fireEvent.blur(field);
    expect(field.value).toBe("8");
    fireEvent.click(screen.getByRole("button", { name: "Start Eagle-Eye" }));
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    expect(startRun.mock.calls[0].slice(8)).toEqual([["api"], 8, null]);
  });

  it("sends no implementer override when the field is untouched", async () => {
    const api = await import("../run/api");
    const detect = api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>;
    const startRun = api.startRun as unknown as ReturnType<typeof vi.fn>;
    startRun.mockReset();
    startRun.mockResolvedValue({ tab: {}, runDir: "/d/runs/new" });
    detect.mockResolvedValue(workspace);
    render(<StartScreen windowId="main" initialFolder="/w" onStarted={() => {}} onPlainTab={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Do it" } });
    await screen.findByRole("checkbox", { name: "web" });
    fireEvent.click(screen.getByRole("button", { name: "Start Eagle-Eye" }));
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    expect(startRun.mock.calls[0].slice(8)).toEqual([["api", "web"], null, null]);
  });
});

describe("StartScreen cost estimate", () => {
  afterEach(cleanup);

  it("shows the range or the not-enough line and never blocks Start", async () => {
    const api = await import("../run/api");
    (api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      kind: "repo",
      repos: [{ name: "app", path: "/w/app", check: null }],
    });
    const overview = api.runOverview as unknown as ReturnType<typeof vi.fn>;
    const finished = (usd: number): RunListItem =>
      run(1, { repoCount: 1, bmadApplied: false, record: { outcome: "done", totalTokens: 1, estimateUsd: usd, minutes: 1, prUrls: [], firstActivity: null, lastActivity: null } });
    overview.mockResolvedValueOnce({ pluginDir: "/p", pluginVersion: null, roles: [], runs: [finished(4), finished(9), finished(6)] });
    const { unmount } = render(<StartScreen windowId="main" initialFolder="/w/app" onStarted={() => {}} onPlainTab={() => {}} />);
    fireEvent.click(await screen.findByLabelText("Off")); // BMAD off: matches the finished runs
    expect((await screen.findByTestId("estimate-line")).textContent).toBe("Similar runs: $4–$9 (3 runs)");
    unmount();

    overview.mockResolvedValueOnce({ pluginDir: "/p", pluginVersion: null, roles: [], runs: [finished(4)] });
    render(<StartScreen windowId="main" initialFolder="/w/app" onStarted={() => {}} onPlainTab={() => {}} />);
    expect((await screen.findByTestId("estimate-line")).textContent).toBe("Not enough earlier runs for an estimate.");
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "Do it" } });
    expect((screen.getByRole("button", { name: "Start Eagle-Eye" }) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("StartScreen refresh stale", () => {
  afterEach(cleanup);

  it("launches onboard with the explicit request, not the textarea text", async () => {
    const api = await import("../run/api");
    const startRun = api.startRun as unknown as ReturnType<typeof vi.fn>;
    startRun.mockReset();
    startRun.mockResolvedValue({ tab: {}, runDir: null });
    (api.detectFolderRepos as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      kind: "repo",
      repos: [{ name: "app", path: "/w/app", check: null }],
    });
    render(<StartScreen windowId="main" initialFolder="/w/app" onStarted={() => {}} onPlainTab={() => {}} />);
    fireEvent.change(screen.getByPlaceholderText(/Describe the feature/), { target: { value: "something else" } });
    fireEvent.click(await screen.findByRole("button", { name: "mock refresh stale" }));
    await vi.waitFor(() => expect(startRun).toHaveBeenCalledOnce());
    expect(startRun.mock.calls[0].slice(1, 4)).toEqual(["/w/app", "stale backend", "onboard"]);
  });
});
