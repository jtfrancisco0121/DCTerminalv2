// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { FAKE_BANNER, FAKE_MARKER, fakeAnswer, installFakeBackend } from "./install";

type Internals = {
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  transformCallback: (
    callback?: (payload: unknown) => void,
    once?: boolean,
  ) => number;
};

function installed() {
  const target = {} as Window & { __TAURI_INTERNALS__?: Internals };
  installFakeBackend(target);
  return target.__TAURI_INTERNALS__ as Internals;
}

describe("the faked backend", () => {
  it("answers a faked command from its fixture", async () => {
    const internals = installed();
    const ctx = (await internals.invoke("window_context")) as {
      id: string;
      accountName: string;
    };
    expect(ctx.id).toBe("main");
    expect(ctx.accountName).toBe("Personal");
    const run = (await internals.invoke("read_run", { runDir: "/x" })) as {
      prs: Array<{ qa?: { result: string } }>;
    };
    expect(run.prs[0].qa?.result).toBe("pass");
  });

  it("rejects a command without a fixture, naming it", async () => {
    const internals = installed();
    await expect(internals.invoke("no_such_command")).rejects.toThrow(
      "fake: no_such_command not faked",
    );
    expect(() => fakeAnswer("start_run")).toThrow("fake: start_run not faked");
  });

  it("hands out a copy so a component cannot change the fixture", async () => {
    const internals = installed();
    const first = (await internals.invoke("get_app_state")) as {
      tabs: unknown[];
    };
    first.tabs.push({ id: "x" });
    const second = (await internals.invoke("get_app_state")) as {
      tabs: unknown[];
    };
    expect(second.tabs).toEqual([]);
  });

  it("delivers an emitted event to a listener and stops after unlisten", async () => {
    const internals = installed();
    const seen: unknown[] = [];
    const handler = internals.transformCallback((e) => seen.push(e));
    const id = (await internals.invoke("plugin:event|listen", {
      event: "tabs-changed",
      handler,
    })) as number;
    await internals.invoke("plugin:event|emit", {
      event: "tabs-changed",
      payload: "tab_1",
    });
    expect(seen).toHaveLength(1);
    expect((seen[0] as { payload: string }).payload).toBe("tab_1");
    await internals.invoke("plugin:event|unlisten", {
      event: "tabs-changed",
      eventId: id,
    });
    await internals.invoke("plugin:event|emit", {
      event: "tabs-changed",
      payload: "tab_2",
    });
    expect(seen).toHaveLength(1);
  });

  it("marks the page so QA can tell the fake from the app", () => {
    const target = {} as Window & { __DCV2_FAKE__?: string };
    installFakeBackend(target);
    expect(target.__DCV2_FAKE__).toBe(FAKE_MARKER);
  });
});

describe("the faked terminal and the Runs data", () => {
  it("delivers a banner packet through the pty_open channel callback", async () => {
    const internals = installed();
    const seen: Array<{ message: { kind: string; data: string } }> = [];
    const id = internals.transformCallback((p) => seen.push(p as never));
    const pid = await internals.invoke("pty_open", { input: { id: "tab_1" }, onOutput: { id } });
    expect(pid).toBe(1);
    await new Promise((r) => setTimeout(r, 5));
    expect(seen).toHaveLength(1);
    expect(seen[0]!.message.kind).toBe("output");
    expect(atob(seen[0]!.message.data)).toBe(FAKE_BANNER);
    expect(await internals.invoke("pty_write", { id: "tab_1", data: "x" })).toBeNull();
  });

  it("answers dispatch_overview with the real shape: holders, a queue item and the cap", async () => {
    const overview = (await installed().invoke("dispatch_overview")) as {
      cap: number;
      holders: unknown[];
      queue: unknown[];
      atCap: boolean;
    };
    expect(overview.holders).toHaveLength(1);
    expect(overview.queue).toHaveLength(1);
    expect(overview.cap).toBeGreaterThan(0);
    expect(overview.atCap).toBe(false);
  });

  it("lists role tiles and earlier runs, and resume_run opens the run's tab", async () => {
    const internals = installed();
    const overview = (await internals.invoke("run_overview")) as { roles: unknown[]; runs: unknown[] };
    expect(overview.roles.length).toBeGreaterThan(0);
    expect(overview.runs.length).toBeGreaterThan(1);
    expect(((await internals.invoke("get_app_state")) as { tabs: unknown[] }).tabs).toEqual([]);
    const started = (await internals.invoke("resume_run", { windowId: "main", runDir: "/x" })) as { tab: { id: string } };
    const after = (await internals.invoke("get_app_state")) as { activeTabId: string; tabs: Array<{ id: string }> };
    expect(after.activeTabId).toBe(started.tab.id);
    expect(after.tabs.map((t) => t.id)).toEqual([started.tab.id]);
  });
});
