// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { TabSummary } from "../bridge";
import { TabBar } from "./TabBar";

const tab = (id: string, label: string, kind: TabSummary["kind"] = "claude"): TabSummary => ({
  id,
  label,
  customLabel: false,
  cwd: `/w/${id}`,
  kind,
  folderStatus: "ok",
  hasSession: false,
});

function bar(exited: string[] = []) {
  const props = {
    tabs: [tab("a", "Claude · a"), tab("b", "Shell · b", "shell")],
    activeTabId: "a",
    exited: new Set(exited),
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onRename: vi.fn(),
    onNew: vi.fn(),
  };
  render(<TabBar {...props} />);
  return props;
}

describe("TabBar", () => {
  it("shows the tabs, the active one selected, and selects, closes and adds", () => {
    const props = bar(["b"]);
    const [first, second] = screen.getAllByRole("tab");
    expect(first.getAttribute("aria-selected")).toBe("true");
    expect(second.closest(".tab")!.className).toContain("tab-exited");
    fireEvent.click(second);
    fireEvent.click(screen.getByRole("button", { name: "Close Claude · a" }));
    fireEvent.click(screen.getByRole("button", { name: "New tab" }));
    expect(props.onSelect).toHaveBeenCalledWith("b");
    expect(props.onClose).toHaveBeenCalledWith("a");
    expect(props.onNew).toHaveBeenCalledOnce();
  });

  it("renames on double-click; Enter saves, Escape and an unchanged name do not", () => {
    const props = bar();
    fireEvent.doubleClick(screen.getAllByRole("tab")[0]);
    const input = screen.getByRole("textbox", { name: "Tab name" });
    fireEvent.change(input, { target: { value: "  Auth bug " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(props.onRename).toHaveBeenCalledWith("a", "Auth bug");
    fireEvent.doubleClick(screen.getAllByRole("tab")[1]);
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Tab name" }), { key: "Escape" });
    fireEvent.doubleClick(screen.getAllByRole("tab")[1]);
    fireEvent.blur(screen.getByRole("textbox", { name: "Tab name" }));
    expect(props.onRename).toHaveBeenCalledOnce();
  });
});
