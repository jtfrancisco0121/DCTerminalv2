// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Failure, FlightEvent, SessionView } from "../flight/types";

const flightSession = vi.fn();
const flightContent = vi.fn();
const flightReplayTrack = vi.fn();
const flightReplayFile = vi.fn();
vi.mock("../flight/api", () => ({
  flightSession: (...args: unknown[]) => flightSession(...args),
  flightContent: (...args: unknown[]) => flightContent(...args),
  flightReplayTrack: (...args: unknown[]) => flightReplayTrack(...args),
  flightReplayFile: (...args: unknown[]) => flightReplayFile(...args),
}));

const { TracePanel } = await import("./TracePanel");

const FILE = "/config/projects/-work-toy/5b0d.jsonl";
let offset = 0;
function ev(
  id: string,
  kind: string,
  at: string,
  extra: Partial<FlightEvent> = {},
  agent: Partial<FlightEvent["agent"]> = {},
): FlightEvent {
  offset += 100;
  return {
    schema: 1,
    id,
    trace: "session:s1",
    session: "s1",
    agent: { id: "main", parent: null, kind: "main", type: null, role: null, pr: null, ...agent },
    provider: { name: "claude-code", version: "2.1.287", native: "x" },
    kind,
    at,
    span: null,
    status: null,
    name: null,
    attrs: {},
    usage: null,
    content: { ref: { file: FILE, offset, len: 90, sha256: `sha-${id}` }, policy: "metadata", redactions: 0 },
    derived: null,
    ...extra,
  };
}

const events: FlightEvent[] = [
  ev("e1", "session.start", "2026-10-22T15:32:16.000Z"),
  ev("e2", "turn.start", "2026-10-22T15:32:16.271Z"),
  ev("e3", "llm.call", "2026-10-22T15:32:41.000Z"),
  ev("e4", "tool.start", "2026-10-22T15:32:41.758Z", { span: "t1", name: "Bash" }),
  ev("e5", "tool.blocked", "2026-10-22T15:32:41.826Z", { span: "t1", name: "Bash", status: "blocked" }),
  ev("u1", "x.claude.unknown", "2026-10-22T15:32:42.000Z"),
  ev("u2", "x.claude.unknown", "2026-10-22T15:32:42.100Z"),
  ev("u3", "x.claude.unknown", "2026-10-22T15:32:42.200Z"),
  ev("e6", "llm.call", "2026-10-22T15:37:16.000Z"),
  ev("e7", "tool.start", "2026-10-22T15:37:16.783Z", { span: "t2", name: "Bash" }),
  ev("e8", "tool.end", "2026-10-22T15:37:17.007Z", { span: "t2", name: "Bash", status: "error" }),
];

const failure = (id: string, at: string, label: string, before: string[], extra: Partial<Failure> = {}): Failure => ({
  id,
  agent: "main",
  at,
  name: "Bash",
  kind: "tool.end",
  derived: { label, evidence: [id] },
  before,
  repeatOf: null,
  repeats: [],
  ...extra,
});

function sessionView(over: Partial<SessionView> = {}): SessionView {
  return {
    status: "ok",
    trace: "r-1",
    policy: "metadata",
    agents: [
      {
        id: "main",
        session: "s1",
        kind: "main",
        type: null,
        role: null,
        pr: null,
        prDerived: null,
        description: null,
        spawnSpan: null,
        files: ["/work/toy/src/add.js"],
        children: [
          {
            id: "a1",
            session: "s1",
            kind: "subagent",
            type: "dcv2:implementer",
            role: "implementer",
            pr: "PR1",
            prDerived: { label: "heuristic:description-prefix", evidence: ["e1"] },
            description: "PR1 build",
            spawnSpan: "t9",
            files: [],
            children: [],
            derived: { label: "exact:tool_use_id", evidence: ["e1"] },
          },
        ],
        derived: { label: "exact:session-root", evidence: ["e1"] },
      },
    ],
    failures: [
      failure("e5", "2026-10-22T15:32:41.826Z", "exact:hook-denial:dcv2-guard", ["e1", "e2", "e3", "e4"], {
        kind: "tool.blocked",
      }),
      failure("e8", "2026-10-22T15:37:17.007Z", "exact:is_error", ["e3", "e4", "e5", "e6", "e7"]),
    ],
    page: { events, nextCursor: null },
    notices: [],
    ...over,
  };
}

beforeEach(() => {
  flightSession.mockReset();
  flightContent.mockReset();
  flightReplayTrack.mockReset();
  flightReplayFile.mockReset();
  flightSession.mockResolvedValue(sessionView());
  flightReplayTrack.mockImplementation(async () => ({
    status: "ok",
    truncated: false,
    entries: events.map((e) => ({
      id: e.id,
      at: e.at,
      kind: e.id === "e4" ? "file.change" : e.kind,
      name: e.name,
      status: e.status,
      file: e.id === "e4" ? 1 : null,
      checkSource: null,
      pr: null,
    })),
  }));
});
afterEach(cleanup);

const open = async () => {
  render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
  await screen.findByRole("listbox", { name: "Events" });
};

describe("TracePanel replay", () => {
  const selectedId = () => document.querySelector('[role="option"][aria-selected="true"]')?.id;

  it("is disabled with its reason under policy off and when there are no events", async () => {
    flightSession.mockResolvedValue(sessionView({ status: "off", policy: "off" }));
    render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    await waitFor(() => expect((screen.getByRole("button", { name: "Replay" }) as HTMLButtonElement).disabled).toBe(true));
    expect(screen.getByRole("button", { name: "Replay" }).getAttribute("title")).toContain("off for this account");
    cleanup();
    flightSession.mockResolvedValue(sessionView({ page: { events: [], nextCursor: null } }));
    render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Replay" }).getAttribute("title")).toContain("no events"));
  });

  it("steps through every Event with the keys and the Event list follows; Esc leaves", async () => {
    await open();
    const toggle = screen.getByRole("button", { name: "Replay" });
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(toggle);
    const slider = await screen.findByRole("slider");
    expect(toggle.getAttribute("aria-pressed")).toBe("true");
    expect(slider.getAttribute("aria-valuemin")).toBe("0");
    expect(slider.getAttribute("aria-valuemax")).toBe(String(events.length - 1));
    expect(slider.getAttribute("aria-valuenow")).toBe("0");
    expect(slider.getAttribute("aria-valuetext")).toContain("Session started");
    expect(selectedId()).toBe("trace-row-e1");
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect(slider.getAttribute("aria-valuenow")).toBe("1");
    expect(selectedId()).toBe("trace-row-e2");
    fireEvent.keyDown(slider, { key: "End" });
    expect(slider.getAttribute("aria-valuenow")).toBe(String(events.length - 1));
    expect(selectedId()).toBe("trace-row-e8");
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect(slider.getAttribute("aria-valuenow")).toBe(String(events.length - 1));
    fireEvent.keyDown(slider, { key: "Home" });
    expect(selectedId()).toBe("trace-row-e1");
    fireEvent.keyDown(screen.getByRole("region", { name: "Trace" }), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("slider")).toBeNull());
    expect(toggle.getAttribute("aria-pressed")).toBe("false");
  });

  it("shows the file labelled recorded for a file.change, and the stop message", async () => {
    flightReplayFile.mockResolvedValue({
      status: "ok",
      reason: null,
      path: "src/add.js",
      pr: "PR1",
      text: "let a = 1;\nlet b = 2;\n",
      redactions: 2,
      truncated: false,
      edit: 2,
      edits: 3,
      stopped: { at: 3, reason: "the text this edit replaces is not in the file" },
    });
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Replay" }));
    const slider = await screen.findByRole("slider");
    // e4 is the fourth Event.
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    const pane = await screen.findByTestId("file-pane");
    expect(flightReplayFile).toHaveBeenCalledWith({ runDir: "/runs/r-1" }, "e4");
    await waitFor(() => expect(within(pane).getByTestId("file-pane-text")).toBeTruthy());
    expect(pane.textContent).toContain("recorded");
    expect(pane.textContent).toContain("src/add.js");
    expect(pane.textContent).toContain("after edit 2 of 3");
    expect(pane.textContent).toContain("2 redacted");
    expect(within(pane).getByTestId("file-pane-stopped").textContent).toContain("Cannot reconstruct this file past this edit.");
    expect(pane.textContent).toContain("Changes made by shell commands are not seen.");
    // Moving to another kind of Event takes the pane away.
    fireEvent.keyDown(slider, { key: "ArrowRight" });
    await waitFor(() => expect(screen.queryByTestId("file-pane")).toBeNull());
  });

  it("says why a file cannot be shown", async () => {
    flightReplayFile.mockResolvedValue({
      status: "unavailable",
      reason: "This file is outside every PR worktree of the run.",
      path: null,
      pr: null,
      text: null,
      redactions: 0,
      truncated: false,
      edit: null,
      edits: 0,
      stopped: null,
    });
    await open();
    fireEvent.click(screen.getByRole("button", { name: "Replay" }));
    const slider = await screen.findByRole("slider");
    for (let i = 0; i < 3; i += 1) fireEvent.keyDown(slider, { key: "ArrowRight" });
    expect((await screen.findByTestId("file-pane-reason")).textContent).toContain("outside every PR worktree");
  });
});

describe("TracePanel export", () => {
  it("has an Export button before Search that is disabled when the policy is off", async () => {
    await open();
    const names = screen.getAllByRole("button").map((b) => b.textContent);
    expect(names.indexOf("Export")).toBeGreaterThan(-1);
    expect((screen.getByRole("button", { name: "Export" }) as HTMLButtonElement).disabled).toBe(false);
    cleanup();
    flightSession.mockResolvedValue(sessionView({ status: "off", policy: "off" }));
    render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    await waitFor(() => expect((screen.getByRole("button", { name: "Export" }) as HTMLButtonElement).disabled).toBe(true));
  });
});

describe("TracePanel snapshot", () => {
  it("shows the Snapshot pill with its time and whether content was kept", async () => {
    flightSession.mockResolvedValue(
      sessionView({ snapshot: { createdAt: "2026-11-05T11:00:00Z", policy: "full", contentKept: true } }),
    );
    await open();
    const pill = screen.getByTestId("trace-snapshot");
    expect(pill.textContent).toBe("Snapshot");
    expect(pill.getAttribute("title")).toContain("content kept");
    expect(pill.getAttribute("title")).toContain("policy full");
  });

  it("has no pill for a live trace, and says 'Content not kept (policy metadata)' for a line the snapshot did not keep", async () => {
    await open();
    expect(screen.queryByTestId("trace-snapshot")).toBeNull();
    cleanup();
    flightSession.mockResolvedValue(
      sessionView({ snapshot: { createdAt: "2026-11-05T11:00:00Z", policy: "metadata", contentKept: false } }),
    );
    flightContent.mockRejectedValue("content not kept");
    await open();
    expect(screen.getByTestId("trace-snapshot").getAttribute("title")).toContain("content not kept");
    const listbox = screen.getByRole("listbox", { name: "Events" });
    fireEvent.keyDown(listbox, { key: "]" });
    await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-e5"));
    fireEvent.keyDown(listbox, { key: "Enter" });
    expect(await screen.findByText("Content not kept (policy metadata)")).toBeTruthy();
  });
});

describe("TracePanel", () => {
  it("shows six placeholder rows while the first page loads", async () => {
    flightSession.mockReturnValue(new Promise(() => {}));
    render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    const loading = screen.getByTestId("trace-loading");
    expect(loading.getAttribute("aria-busy")).toBe("true");
    expect(loading.children).toHaveLength(6);
  });

  it("lists two failures, ] selects the first, its context band shows the events before it, Enter reads the content once", async () => {
    flightContent.mockResolvedValue({ text: 'before <b>bold</b> after\nline 2', redactions: 1, policy: "metadata" });
    await open();
    const rail = screen.getByLabelText("Failures and agents");
    expect(within(rail).getAllByRole("button", { name: /Bash/ })).toHaveLength(2);

    const listbox = screen.getByRole("listbox", { name: "Events" });
    fireEvent.keyDown(listbox, { key: "]" });
    await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-e5"));
    const band = screen.getByTestId("trace-context");
    expect(within(band).getAllByRole("listitem")).toHaveLength(4);
    expect(band.textContent).toContain("Session started");
    expect(band.textContent).toContain("Tool call: Bash");

    expect(flightContent).not.toHaveBeenCalled();
    fireEvent.keyDown(listbox, { key: "Enter" });
    const pre = await waitFor(() => {
      const el = screen.getByTestId("trace-content").querySelector("pre");
      expect(el).toBeTruthy();
      return el!;
    });
    expect(flightContent).toHaveBeenCalledTimes(1);
    expect(flightContent.mock.calls[0]![1]).toEqual(events.find((e) => e.id === "e5")!.content.ref);
    // Plain text: the tag is shown as characters, not rendered.
    expect(pre.textContent).toBe("before <b>bold</b> after\nline 2");
    expect(pre.querySelector("b")).toBeNull();
    expect(screen.getByTestId("trace-content").textContent).toContain("1 redacted");

    // ] again moves to the second failure; [ goes back.
    fireEvent.keyDown(listbox, { key: "]" });
    await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-e8"));
    fireEvent.keyDown(listbox, { key: "[" });
    await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-e5"));
  });

  it("moves with the arrow keys and collapses consecutive unknown lines", async () => {
    await open();
    expect(screen.getByText("3 unknown lines")).toBeTruthy();
    expect(screen.getAllByRole("option")).toHaveLength(9);
    const listbox = screen.getByRole("listbox", { name: "Events" });
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-e1");
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    fireEvent.keyDown(listbox, { key: "ArrowUp" });
    expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-e1");
  });

  it("marks a repeat as heuristic and nests it under its failure", async () => {
    const first = failure("e5", "2026-10-22T15:32:41.826Z", "exact:is_error", ["e4"]);
    const repeat = failure("e8", "2026-10-22T15:37:17.007Z", "exact:is_error", ["e7"], {
      repeatOf: { label: "heuristic:same-tool-same-input-after-error", evidence: ["e5", "e8"] },
    });
    flightSession.mockResolvedValue(sessionView({ failures: [{ ...first, repeats: [repeat] }] }));
    await open();
    const rail = screen.getByLabelText("Failures and agents");
    expect(within(rail).getByRole("button", { name: /Repeat of \d\d:\d\d:\d\d/ }).textContent).toContain("heuristic");
  });

  it("makes no content call and shows the Settings hint when the policy is off", async () => {
    flightSession.mockResolvedValue(
      sessionView({ status: "off", policy: "off", agents: [], failures: [], page: { events: [], nextCursor: null } }),
    );
    render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    const note = await screen.findByTestId("trace-off");
    expect(note.textContent).toContain("Settings");
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(flightContent).not.toHaveBeenCalled();
  });

  it("says the transcript is gone, and shows an unknown folder as a read error with a retry", async () => {
    flightSession.mockResolvedValue(
      sessionView({ notices: ["transcript gone: abc"], page: { events: [], nextCursor: null }, failures: [] }),
    );
    const { unmount } = render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    expect((await screen.findByTestId("trace-gone")).textContent).toContain("gone");
    unmount();
    flightSession.mockReset();
    flightSession.mockRejectedValue("unknown account folder");
    render(<TracePanel target={{ configDir: "/nope", sessionId: "s" }} onClose={() => {}} />);
    const error = await screen.findByTestId("trace-error");
    expect(error.textContent).toContain("unknown account folder");
    expect(within(error).getByRole("button", { name: "Try again" })).toBeTruthy();
  });

  it("says when there is nothing yet", async () => {
    flightSession.mockResolvedValue(sessionView({ page: { events: [], nextCursor: null }, failures: [] }));
    render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
    expect(await screen.findByText(/No events were recorded/)).toBeTruthy();
  });

  it("loads the next page on demand and shows the agent tree with files", async () => {
    flightSession
      .mockResolvedValueOnce(sessionView({ page: { events: events.slice(0, 4), nextCursor: "c1" } }))
      .mockResolvedValueOnce(sessionView({ page: { events: events.slice(4), nextCursor: null } }));
    await open();
    expect(screen.getAllByRole("option")).toHaveLength(4);
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(4));
    expect(flightSession.mock.calls[1]![0].cursor).toBe("c1");
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
    expect(screen.getByText("Files orchestrator touched")).toBeTruthy();
    expect(screen.getByText("/work/toy/src/add.js")).toBeTruthy();
    expect(screen.getByRole("button", { name: /implementer/ }).textContent).toContain("PR1");
  });

  it("filters to one agent from the tree and opens on a given agent", async () => {
    render(<TracePanel target={{ runDir: "/runs/r-1" }} agent="a1" onClose={() => {}} />);
    await screen.findByRole("listbox", { name: "Events" });
    expect(flightSession.mock.calls[0]![0].agent).toBe("a1");
    fireEvent.click(screen.getByRole("button", { name: "All agents" }));
    await waitFor(() => expect(flightSession.mock.calls[flightSession.mock.calls.length - 1]![0].agent).toBeUndefined());
  });

  it("Escape first closes an open content view", async () => {
    flightContent.mockResolvedValue({ text: "hello", redactions: 0, policy: "metadata" });
    await open();
    const listbox = screen.getByRole("listbox", { name: "Events" });
    fireEvent.keyDown(listbox, { key: "ArrowDown" });
    fireEvent.keyDown(listbox, { key: "Enter" });
    await screen.findByTestId("trace-content");
    fireEvent.keyDown(listbox, { key: "Escape" });
    expect(screen.queryByTestId("trace-content")).toBeNull();
  });

  describe("the run's own lane", () => {
    const RUN_FILE = "/runs/r-1/log.md";
    function run(
      id: string,
      kind: string,
      at: string,
      attrs: Record<string, unknown> = {},
      extra: Partial<FlightEvent> = {},
    ): FlightEvent {
      return ev(
        id,
        kind,
        at,
        {
          provider: { name: "dcv2", version: null, native: "log.md" },
          attrs,
          content: { ref: { file: RUN_FILE, offset: 0, len: 10, sha256: `sha-${id}` }, policy: "full", redactions: 0 },
          ...extra,
        },
        { id: "orchestration", kind: "orchestration", parent: null },
      );
    }

    const mixed: FlightEvent[] = [
      run("o1", "stage.change", "2026-11-05T10:07:00.000Z", { to: "building" }, { name: "building" }),
      run("o2", "approval.wait", "2026-11-05T10:08:00.000Z", {}, { name: "plan" }),
      run("o3", "approval.decision", "2026-11-05T10:09:00.000Z", {}, { name: "plan", status: "ok" }),
      run("o4", "review.round", "2026-11-05T10:10:00.000Z", { pr: "PR2", round: 1, verdict: "FAIL" }, { name: "PR2-round1" }),
      run("o5", "check.end", "2026-11-05T10:11:00.000Z", { source: "app", pr: "PR2" }, { name: "cargo test", status: "fail" }),
      run("o6", "check.end", "2026-11-05T10:12:00.000Z", { source: "stop-check", pr: "PR2" }, { name: "cargo test", status: "fail" }),
      run("o7", "x.dcv2.pr-state", "2026-11-05T10:13:00.000Z", { pr: "PR2", from: "reviewing", to: "fixing" }, { name: "fixing" }),
      ev("t1", "tool.blocked", "2026-11-05T10:14:00.000Z", { span: "t9", name: "Bash", status: "blocked", attrs: { rule: "run-file-write" } }, { id: "a1", role: "implementer", pr: "PR2" }),
      ev("t2", "tool.start", "2026-11-05T10:15:00.000Z", { name: "Edit" }, { id: "a1", role: "implementer", pr: "PR2" }),
      ev("t3", "tool.start", "2026-11-05T10:16:00.000Z", { name: "Read" }, { id: "main" }),
    ];
    const withLane = (over: Partial<SessionView> = {}) =>
      sessionView({
        page: { events: mixed, nextCursor: null },
        failures: [
          failure("o5", "2026-11-05T10:11:00.000Z", "exact:check-fail", ["o4"], { name: "cargo test", kind: "check.end", agent: "orchestration" }),
          failure("t1", "2026-11-05T10:14:00.000Z", "exact:hook-denial:dcv2-guard", [], { kind: "tool.blocked", agent: "a1", rule: "run-file-write" }),
        ],
        agents: [
          {
            id: "main", session: "s1", kind: "main", type: null, role: null, pr: null, prDerived: null, description: null, spawnSpan: null, files: [],
            derived: { label: "exact:session-root", evidence: ["t3"] },
            children: [
              { id: "a1", session: "s1", kind: "subagent", type: "dcv2:implementer", role: "implementer", pr: "PR2", prDerived: { label: "exact:pr-description", evidence: ["t2"] }, description: "PR2 implement", spawnSpan: "t8", files: [], children: [], derived: { label: "exact:tool_use_id", evidence: ["t2"] } },
              { id: "a2", session: "s1", kind: "subagent", type: "dcv2:pr-reviewer", role: "pr-reviewer", pr: "PR3", prDerived: { label: "heuristic:description-prefix", evidence: ["t2"] }, description: "PR3 review", spawnSpan: "t7", files: [], children: [], derived: { label: "exact:tool_use_id", evidence: ["t2"] } },
            ],
          },
        ],
        ...over,
      });

    const openRun = async (view = withLane()) => {
      flightSession.mockResolvedValue(view);
      await open();
    };

    it("shows the orchestration rows in the app's words beside the agent rows", async () => {
      await openRun();
      const list = screen.getByRole("listbox", { name: "Events" });
      const text = list.textContent ?? "";
      for (const words of [
        "Stage: building",
        "Waiting for plan approval",
        "Plan approved",
        "PR2 review round 1",
        "Check (app) PR2 failed",
        "stop-check PR2 failed",
        "pr PR2 reviewing → fixing",
        "Tool blocked: Bash",
      ]) {
        expect(text, words).toContain(words);
      }
      expect(within(list).getAllByText("run").length).toBe(7);
      expect(document.getElementById("trace-row-o1")?.className).toContain("trace-row-run");
      expect(document.getElementById("trace-row-t2")?.className).not.toContain("trace-row-run");
    });

    it("has a lane filter with arrow keys for a run, and none for a session", async () => {
      await openRun();
      const group = screen.getByRole("radiogroup", { name: "Lanes" });
      const radios = within(group).getAllByRole("radio");
      expect(radios.map((r) => r.textContent)).toEqual(["Agents", "Orchestration", "Both"]);
      expect(radios[2]!.getAttribute("aria-checked")).toBe("true");
      const count = () => screen.getAllByRole("option").length;
      expect(count()).toBe(10);
      fireEvent.click(radios[0]!);
      expect(count()).toBe(3);
      expect(screen.queryByText(/Stage: building/)).toBeNull();
      fireEvent.click(radios[1]!);
      expect(count()).toBe(7);
      expect(screen.queryByText(/Tool blocked: Bash/)).toBeNull();
      fireEvent.keyDown(group, { key: "ArrowRight" });
      expect(screen.getByRole("radio", { name: "Both" }).getAttribute("aria-checked")).toBe("true");
      expect(count()).toBe(10);
      fireEvent.keyDown(group, { key: "ArrowLeft" });
      fireEvent.keyDown(group, { key: "ArrowLeft" });
      expect(screen.getByRole("radio", { name: "Agents" }).getAttribute("aria-checked")).toBe("true");
      cleanup();
      flightSession.mockResolvedValue(withLane());
      render(<TracePanel target={{ configDir: "/c", sessionId: "s1" }} onClose={() => {}} />);
      await screen.findByRole("listbox", { name: "Events" });
      expect(screen.queryByRole("radiogroup", { name: "Lanes" })).toBeNull();
    });

    it("draws an exact PR pill solid and a heuristic one dashed, saying heuristic, with a tooltip", async () => {
      await openRun();
      const rail = screen.getByLabelText("Failures and agents");
      const exact = within(rail).getByText("PR2");
      expect(exact.className).toContain("pill");
      expect(exact.className).not.toContain("pill-heuristic");
      const guess = within(rail).getByText("PR3 · heuristic");
      expect(guess.className).toContain("pill-heuristic");
      expect(guess.getAttribute("tabindex")).toBe("0");
      expect(guess.getAttribute("data-tip")).toMatch(/Guessed/);
    });

    it("shows the guard rule id on the failure entry and on the block row", async () => {
      await openRun();
      const rail = screen.getByLabelText("Failures and agents");
      const rule = within(rail).getByText("run-file-write");
      expect(rule.tagName).toBe("CODE");
      expect(rule.className).toContain("tone-warn");
      const row = document.getElementById("trace-row-t1")!;
      expect(within(row).getByText("run-file-write").className).toContain("trace-rule");
    });

    it("] reaches a failed check of the run's lane", async () => {
      await openRun();
      const listbox = screen.getByRole("listbox", { name: "Events" });
      fireEvent.keyDown(listbox, { key: "]" });
      await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-o5"));
      fireEvent.keyDown(listbox, { key: "]" });
      await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-t1"));
    });

    it("filters to a PR from a row, and a failure that the filter hides brings everything back", async () => {
      await openRun();
      fireEvent.click(within(document.getElementById("trace-row-o4")!).getByRole("button", { name: "Show only PR2" }));
      expect(screen.getByTestId("trace-pr-filter").textContent).toContain("PR2");
      // PR2's rows and PR2's agent; not the stage change, the main session or other PRs.
      expect(screen.getAllByRole("option").map((o) => o.id)).toEqual([
        "trace-row-o4", "trace-row-o5", "trace-row-o6", "trace-row-o7", "trace-row-t1", "trace-row-t2",
      ]);
      fireEvent.click(screen.getByRole("button", { name: "Show all PRs" }));
      expect(screen.getAllByRole("option")).toHaveLength(10);
    });

    it("reads a row's content as plain text with its fields and its phase", async () => {
      flightContent.mockResolvedValue({ text: "line one\n<b>two</b>", redactions: 0, policy: "full" });
      await openRun(withLane({ phases: { o5: { stage: "building", prState: "reviewing", label: "exact:log-line" } } }));
      fireEvent.click(document.getElementById("trace-row-o5")!);
      const box = await screen.findByTestId("trace-content");
      await waitFor(() => expect(box.querySelector("pre")).toBeTruthy());
      expect(box.querySelector("pre")!.textContent).toBe("line one\n<b>two</b>");
      expect(box.querySelector("pre")!.className).toContain("trace-pre-run");
      const fields = within(box).getByTestId("trace-fields").textContent ?? "";
      expect(fields).toContain("source");
      expect(fields).toContain("app");
      expect(fields).toContain("stage building · PR reviewing");
      expect(flightContent.mock.calls[0]![1]).toEqual(mixed.find((e) => e.id === "o5")!.content.ref);
    });

    it("shows a malformed-file notice inline", async () => {
      await openRun(withLane({ notices: ["status.json: malformed JSON, its events are left out"] }));
      expect(screen.getByTestId("trace-notice").textContent).toContain("status.json");
    });

    it("opens on the page of a focused event and selects it once", async () => {
      flightSession.mockResolvedValue(withLane());
      render(<TracePanel target={{ runDir: "/runs/r-1" }} focus="o5" onClose={() => {}} />);
      const listbox = await screen.findByRole("listbox", { name: "Events" });
      expect(flightSession.mock.calls[0]![0].focus).toBe("o5");
      await waitFor(() => expect(listbox.getAttribute("aria-activedescendant")).toBe("trace-row-o5"));
      fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
      await waitFor(() => expect(flightSession).toHaveBeenCalledTimes(2));
      expect(flightSession.mock.calls[1]![0].focus).toBeUndefined();
    });

    it("has a Search button when it can open the search page", async () => {
      const onSearch = vi.fn();
      flightSession.mockResolvedValue(withLane());
      render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} onSearch={onSearch} />);
      await screen.findByRole("listbox", { name: "Events" });
      fireEvent.click(screen.getByRole("button", { name: "Search" }));
      expect(onSearch).toHaveBeenCalledTimes(1);
    });
  });
});
