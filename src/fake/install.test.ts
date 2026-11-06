// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { FAKE_MARKER, fakeAnswer, installFakeBackend } from "./install";

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
