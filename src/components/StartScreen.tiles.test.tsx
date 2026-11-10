// @vitest-environment jsdom
// The start screen's role tiles on the faked backend's overview: labels, groups and order come from the fixture, which a
// Rust test holds equal to `ROLES` (run/launch.rs), so no role id is written here. Recorded exception (decision log,
// 2026-11-10): the tile count (13, in groups of 8 and 5) is written literally, to catch a persona dropped from `ROLES`
// and the fixture together, which the equality test cannot see.
import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RunOverview } from "../run/api";

// Read, not imported: nothing under src/ outside src/fake imports the faked backend's files (noImport.test.ts).
const raw = import.meta.glob("/fixtures/fake/runs.json", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;
const fixture = JSON.parse(Object.values(raw)[0]) as { run_overview: RunOverview };

vi.mock("../run/api", async () => ({
  duplicateOf: () => null,
  runOverview: vi.fn(async () => fixture.run_overview),
  detectFolderRepos: vi.fn(async () => ({ kind: "none", repos: [] })),
  startRun: vi.fn(),
  resumeRun: vi.fn(),
  readRunFile: vi.fn(),
}));
vi.mock("../bridge", () => ({
  recentFolders: vi.fn(async () => []),
  getSettings: vi.fn(async () => ({ run: { maxImplementers: 3, stopHooks: true } })),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ revealItemInDir: vi.fn(), openUrl: vi.fn() }));
vi.mock("./WorktreeSetup", () => ({ WorktreeSetup: () => null }));
vi.mock("./MemoryPanel", () => ({ MemoryPanel: () => null }));
vi.mock("../clickup/ClickUpIntake", () => ({ ClickUpIntake: () => null }));

const { StartScreen } = await import("./StartScreen");

const roles = fixture.run_overview.roles;
/** The groups of the fixture in first-seen order, each with its tiles' labels in order. */
const expectedGroups = roles.reduce<Array<{ group: string; labels: string[] }>>((groups, role) => {
  const found = groups.find((g) => g.group === role.group);
  if (found) found.labels.push(role.label);
  else groups.push({ group: role.group, labels: [role.label] });
  return groups;
}, []);

const renderStart = (initialFolder: string) =>
  render(<StartScreen windowId="main" initialFolder={initialFolder} onStarted={() => {}} onPlainTab={() => {}} />);

/** The role groups: the `role="group"` elements that hold tiles. */
const tileGroups = () => screen.getAllByRole("group").filter((el) => el.querySelector(".role-tile"));

afterEach(cleanup);

describe("StartScreen role tiles", () => {
  it("has the faked overview's roles in the fixture's groups", () => {
    expect(roles.length).toBeGreaterThan(0);
    expect(expectedGroups.length).toBe(2);
    expect(roles).toHaveLength(13);
    expect(expectedGroups.map((g) => g.labels.length)).toEqual([8, 5]);
  });

  it("shows no tile and no heading before the overview loads", () => {
    renderStart("");
    expect(document.querySelector(".role-tile")).toBeNull();
    expect(document.querySelector(".role-group-heading")).toBeNull();
  });

  it("shows one tile per role in labelled groups, in the fixture's order", async () => {
    renderStart("/w/app");
    await vi.waitFor(() => expect(tileGroups()).toHaveLength(expectedGroups.length));
    expect(document.querySelectorAll(".role-tile")).toHaveLength(13);
    const groups = tileGroups();
    expect(groups.map((g) => g.querySelectorAll(".role-tile").length)).toEqual([8, 5]);
    groups.forEach((groupEl, at) => {
      const heading = document.getElementById(groupEl.getAttribute("aria-labelledby") ?? "");
      expect(heading?.textContent?.trim()).toBeTruthy();
      // The group's accessible name is its heading.
      expect(screen.getByRole("group", { name: heading!.textContent! })).toBe(groupEl);
      const labels = within(groupEl)
        .getAllByRole("button")
        .map((b) => b.textContent);
      expect(labels).toEqual(expectedGroups[at].labels);
    });
    // The headings differ and keep the groups' order in the page.
    const names = groups.map((g) => document.getElementById(g.getAttribute("aria-labelledby")!)!.textContent);
    expect(new Set(names).size).toBe(groups.length);
    expect(groups[0].compareDocumentPosition(groups[1]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("disables every tile until a folder is chosen", async () => {
    renderStart("");
    await vi.waitFor(() => expect(document.querySelectorAll(".role-tile")).toHaveLength(roles.length));
    for (const tile of document.querySelectorAll<HTMLButtonElement>(".role-tile")) expect(tile.disabled).toBe(true);
  });

  it("enables the tiles once there is a folder", async () => {
    renderStart("/w/app");
    await vi.waitFor(() => expect(document.querySelectorAll(".role-tile")).toHaveLength(roles.length));
    for (const tile of document.querySelectorAll<HTMLButtonElement>(".role-tile")) expect(tile.disabled).toBe(false);
  });
});
