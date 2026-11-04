// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Failure, FlightEvent, SessionView } from "../flight/types";

const flightSession = vi.fn();
const flightContent = vi.fn();
vi.mock("../flight/api", () => ({
  flightSession: (...args: unknown[]) => flightSession(...args),
  flightContent: (...args: unknown[]) => flightContent(...args),
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
  flightSession.mockResolvedValue(sessionView());
});
afterEach(cleanup);

const open = async () => {
  render(<TracePanel target={{ runDir: "/runs/r-1" }} onClose={() => {}} />);
  await screen.findByRole("listbox", { name: "Events" });
};

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
});
