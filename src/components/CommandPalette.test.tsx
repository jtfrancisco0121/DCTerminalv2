// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearUiErrors, uiErrors } from "../uiErrors";
import { CommandPalette, matchRank, rankItems, type PaletteItem } from "./CommandPalette";

const item = (id: string, group: PaletteItem["group"], label: string, run: PaletteItem["run"] = vi.fn()): PaletteItem => ({ id, group, label, run });

describe("ranking", () => {
  it("matches a case-insensitive subsequence: prefix, then word start, then other", () => {
    expect(matchRank("Settings: Terminal", "set")).toBe(0);
    expect(matchRank("Resume: Add multiply()", "mul")).toBe(1);
    expect(matchRank("Claude accounts", "cnt")).toBe(2);
    expect(matchRank("Terminal", "xyz")).toBeNull();
    expect(matchRank("Terminal", "")).toBe(2);
    const items = [
      item("a", "Settings", "Settings: Terminal"),
      item("b", "Tabs", "my term shell"),
      item("c", "Roles", "Start Planner"),
      item("d", "Tabs", "Terminal tab"),
    ];
    expect(rankItems(items, "term").map((i) => i.id)).toEqual(["d", "b", "a"]);
    expect(rankItems(items, "zzz")).toEqual([]);
  });

  it("keeps the group order on ties and lists at most 50", () => {
    const items = [item("s", "Settings", "alpha"), item("t", "Tabs", "alpha"), item("r", "Roles", "alpha")];
    expect(rankItems(items, "alp").map((i) => i.id)).toEqual(["t", "r", "s"]);
    const many = Array.from({ length: 80 }, (_, i) => item(`i${i}`, "Tabs", `tab ${i}`));
    expect(rankItems(many, "")).toHaveLength(50);
  });
});

describe("CommandPalette", () => {
  beforeEach(() => clearUiErrors());
  afterEach(cleanup);

  const items = (run = vi.fn()): PaletteItem[] => [
    item("t1", "Tabs", "Claude · app", run),
    item("t2", "Tabs", "Shell · api"),
    item("s1", "Settings", "Settings: Terminal"),
  ];

  it("has the combobox and listbox roles and filters as you type", () => {
    render(<CommandPalette items={items()} onClose={() => {}} />);
    expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
    const box = screen.getByRole("combobox");
    expect(document.activeElement).toBe(box);
    expect(screen.getAllByRole("option")).toHaveLength(3);
    fireEvent.change(box, { target: { value: "term" } });
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["SettingsSettings: Terminal"]);
    expect(box.getAttribute("aria-activedescendant")).toBe(screen.getByRole("option").id);
    fireEvent.change(box, { target: { value: "qqq" } });
    expect(screen.getByText("Nothing matches")).toBeTruthy();
    expect(screen.queryByRole("listbox")).toBeNull();
  });

  it("wraps the arrows and runs the active row with Enter, then closes", async () => {
    const run = vi.fn();
    const onClose = vi.fn();
    render(<CommandPalette items={items(run)} onClose={onClose} />);
    const box = screen.getByRole("combobox");
    const selected = () => screen.getAllByRole("option").findIndex((o) => o.getAttribute("aria-selected") === "true");
    expect(selected()).toBe(0);
    fireEvent.keyDown(box, { key: "ArrowUp" });
    expect(selected()).toBe(2);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    expect(selected()).toBe(0);
    fireEvent.keyDown(box, { key: "ArrowDown" });
    fireEvent.keyDown(box, { key: "ArrowUp" });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    expect(run).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("reports a failing action (with a notice) and still closes", async () => {
    const onClose = vi.fn();
    render(<CommandPalette items={[item("x", "Runs", "Resume: broken", () => Promise.reject(new Error("no such run")))]} onClose={onClose} />);
    await act(async () => {
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" });
    });
    expect(onClose).toHaveBeenCalledOnce();
    expect(uiErrors()[0].message).toContain("no such run");
  });

  it("Escape closes it and focus returns to where it was", () => {
    const outside = document.createElement("button");
    document.body.append(outside);
    outside.focus();
    const onClose = vi.fn();
    const { rerender } = render(<CommandPalette items={items()} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Command palette" }), { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
    rerender(<div />);
    expect(document.activeElement).toBe(outside);
    outside.remove();
  });

  it("clicking a row runs it", async () => {
    const run = vi.fn();
    render(<CommandPalette items={items(run)} onClose={() => {}} />);
    await act(async () => {
      fireEvent.click(screen.getAllByRole("option")[0]);
    });
    expect(run).toHaveBeenCalledOnce();
  });
});
