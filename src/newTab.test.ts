import { describe, expect, it } from "vitest";
import type { AppStateSnapshot, TabSummary } from "./bridge";
import { newTabDefaults } from "./newTab";

const tab = (id: string, cwd: string, kind: TabSummary["kind"] = "claude"): TabSummary => ({
  id,
  label: id,
  customLabel: false,
  cwd,
  kind,
  folderStatus: "ok",
  hasSession: false,
});

const state = (tabs: TabSummary[], activeTabId: string | null): AppStateSnapshot => ({
  windowId: "main",
  activeTabId,
  tabs,
  closedTabs: [],
});

describe("new tab defaults", () => {
  it("start in the active tab's folder, as the same kind", () => {
    const tabs = [tab("a", "/w/a"), tab("b", "/w/b", "shell")];
    expect(newTabDefaults(state(tabs, "b"))).toEqual({ cwd: "/w/b", kind: "shell" });
    expect(newTabDefaults(state(tabs, "a"))).toEqual({ cwd: "/w/a", kind: "claude" });
  });

  it("fall back to the newest tab, then the last folder, as a Claude tab", () => {
    expect(newTabDefaults(state([tab("a", "/w/a", "shell")], null))).toEqual({ cwd: "/w/a", kind: "claude" });
    expect(newTabDefaults(state([], null), "/w/last")).toEqual({ cwd: "/w/last", kind: "claude" });
    expect(newTabDefaults(null)).toEqual({ cwd: "", kind: "claude" });
  });
});
