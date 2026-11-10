// @vitest-environment jsdom
// Opens every top-level page and dialog of the app on the faked backend and fails on any command the fake does not answer.
//
// Detection does not rely on visible text: several callers swallow a rejection (UsagePopover turns an observe_tab failure into
// "unavailable", WebQaSection drops a get_settings failure, run/events.ts catches refresh errors), so the fake `invoke` is
// wrapped and every `fake: <command> not faked` rejection is recorded together with the page that was open. The visible
// "not faked" text check is a second signal. Only what a webview owns is replaced (xterm, window handles, native dialogs).
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../components/TerminalView", () => ({
  TerminalView: () => <div data-testid="terminal" />,
  destroyTerminal: vi.fn(),
}));
vi.mock("../terminal/park", () => ({
  focusParkedTerminal: vi.fn(),
  refitTerminal: vi.fn(),
  requestTerminalSearch: vi.fn(),
  setCopyOnSelect: vi.fn(),
  setCompactPaste: vi.fn(),
  setFileLinkEditor: vi.fn(),
  setOptionAsMeta: vi.fn(),
  setTerminalTheme: vi.fn(),
  ensureParkedTerminal: vi.fn(),
  measuredTerminalSize: () => ({ cols: 80, rows: 24 }),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ label: "main", listen: async () => () => {} }),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => {}), revealItemInDir: vi.fn(async () => {}) }));

import App from "../App";
import { EvalDialog } from "../components/EvalDialog";
import { EvalResults } from "../components/EvalResults";
import { installFakeBackend } from "./install";

type Internals = { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> };

/** Every `fake: <command> not faked` rejection, with the page that was open. */
const rejections: Array<{ command: string; page: string }> = [];
/** The pages and dialogs opened, in order. */
const opened: string[] = [];
let page = "start";

function wrapFakeInvoke() {
  const internals = (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
  const real = internals.invoke;
  internals.invoke = async (command, args) => {
    try {
      return await real(command, args);
    } catch (err) {
      if (err instanceof Error && /^fake: .+ not faked$/.test(err.message)) rejections.push({ command, page });
      throw err;
    }
  };
}

/** Lets pending invokes, timers and state updates run. */
async function settle(ms = 60) {
  for (let i = 0; i < 3; i++) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, ms));
    });
  }
}

async function open(name: string, action?: () => void | Promise<void>) {
  page = name;
  opened.push(name);
  if (action) await action();
  await settle();
  // The second signal: the page shows "fake: <command> not faked" itself.
  for (const m of (document.body.textContent ?? "").matchAll(/fake: (\S+) not faked/g)) {
    if (!rejections.some((r) => r.command === m[1] && r.page === name)) rejections.push({ command: m[1]!, page: `${name} (shown as text)` });
  }
}

const click = (el: HTMLElement) => act(async () => void fireEvent.click(el));
// A dialog closes on Escape at its own element; a page listens on the window.
const escape = () =>
  act(async () => {
    const dialogs = document.querySelectorAll('[role="dialog"]');
    fireEvent.keyDown(dialogs.length ? dialogs[dialogs.length - 1]! : window, { key: "Escape" });
  });
const palette = () => act(async () => void fireEvent.keyDown(window, { key: "K", ctrlKey: true, shiftKey: true }));

/** Opens the palette and picks the row whose label is `label`. */
async function viaPalette(label: string) {
  await palette();
  await settle();
  const rows = await screen.findAllByRole("option");
  const row = rows.find((r) => r.querySelector(".palette-label")?.textContent === label);
  expect(row, `palette row "${label}"`).toBeTruthy();
  await click(row!);
}

beforeEach(() => {
  rejections.length = 0;
  opened.length = 0;
  page = "start";
  installFakeBackend();
  wrapFakeInvoke();
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("the app on the faked backend", () => {
  /** A page that fails to show because a command was not answered names the command, not the missing element. */
  const unanswered = () =>
    `the faked backend did not answer: ${rejections.map((r) => `${r.command} (on ${r.page})`).join(", ")}`;

  it("opens every page and dialog without a command the fake does not answer", { timeout: 60000 }, async () => {
    try {
      await walkThePages();
    } catch (err) {
      expect(rejections, unanswered()).toEqual([]);
      throw err;
    }
    expect(rejections, unanswered()).toEqual([]);
  });

  async function walkThePages() {
    render(<App />);
    await screen.findByRole("heading", { name: "New run" });
    await open("start screen");

    // Runs, with an earlier run to resume.
    await open("runs dialog", async () => {
      await click(await screen.findByRole("button", { name: /^Show all/ }));
      await screen.findByRole("dialog");
    });
    const runs = screen.getByRole("dialog");
    await open("resumed run panel", async () => {
      await click((await within(runs).findAllByRole("button", { name: "Resume" }))[0]!);
      await screen.findByLabelText("Session");
    });

    // The run panel: open every disclosure so each section reads what it needs.
    await open("run panel sections", async () => {
      for (const summary of Array.from(document.querySelectorAll(".run-panel summary"))) await click(summary as HTMLElement);
    });
    expect(screen.getByRole("button", { name: "Open trace" }), "the Session section shows its Trace button").toBeTruthy();

    await open("trace", async () => {
      await click(screen.getByRole("button", { name: "Open trace" }));
    });
    await open("export dialog", async () => {
      await click(await screen.findByRole("button", { name: "Export" }));
    });
    await escape();
    await escape();
    await settle();

    for (const [name, label] of [
      ["search traces", "Search traces"],
      ["compare", "Compare runs"],
      ["run dashboard", "Run dashboard"],
      ["run analytics", "Run analytics"],
    ] as const) {
      await open(name, () => viaPalette(label));
      await escape();
    }

    await open("command palette", async () => {
      await palette();
    });
    const sections = (await screen.findAllByRole("option"))
      .map((o) => o.querySelector(".palette-label")?.textContent ?? "")
      .filter((label) => label.startsWith("Settings: "));
    expect(sections.length).toBeGreaterThan(3);
    await escape();
    for (const section of sections) {
      await open(section.replace("Settings: ", "settings "), () => viaPalette(section));
      await escape();
    }
    await open("settings", async () => {
      await click(screen.getByRole("button", { name: "Settings" }));
    });
    await escape();

    // Eval: reached from a finished run when Eval mode is on, so it is mounted directly.
    let evalDialog: ReturnType<typeof render> | undefined;
    await open("eval dialog", async () => {
      evalDialog = render(<EvalDialog windowId="main" runDir="/fake/runs/r-20261022-1304" onClose={() => {}} onQueued={() => {}} />);
    });
    evalDialog?.unmount();
    await open("eval results", async () => {
      render(<EvalResults group="g-fake" onOpenTrace={() => {}} onClose={() => {}} />);
    });

    // The memory panel is on the start screen once a folder with repos is chosen; it is part of that page.
    expect(opened.length).toBeGreaterThan(12);
    // The pages opened are named so a removed step shows in the failure.
    expect(opened).toEqual(expect.arrayContaining(["start screen", "runs dialog", "resumed run panel", "trace", "export dialog", "compare", "eval dialog"]));
  }

  it("records a command the fake does not answer, even when the caller swallows it", async () => {
    const internals = (window as unknown as { __TAURI_INTERNALS__: Internals }).__TAURI_INTERNALS__;
    page = "probe";
    await internals.invoke("no_such_command").catch(() => undefined);
    expect(rejections).toEqual([{ command: "no_such_command", page: "probe" }]);
    rejections.length = 0;
  });
});

