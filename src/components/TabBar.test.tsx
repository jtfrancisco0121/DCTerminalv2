// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { dropIndex, markerX } from "../tabDrag";
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

describe("TabBar activity dot", () => {
  it("a plain Claude tab's dot is a button that asks for its usage; a run tab's is not", () => {
    const onActivityClick = vi.fn();
    render(
      <TabBar
        tabs={[tab("a", "Claude · a"), { ...tab("r", "Eagle-Eye · r"), role: "eagle-eye", runDir: "/d/r" }]}
        activeTabId="a"
        exited={new Set()}
        activity={{ a: "working", r: "working" }}
        onActivityClick={onActivityClick}
        onSelect={vi.fn()}
        onClose={vi.fn()}
        onRename={vi.fn()}
        onNew={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Claude working: show usage" }));
    expect(onActivityClick).toHaveBeenCalledWith("a");
    expect(screen.getAllByLabelText(/^Claude working/)).toHaveLength(2);
    expect(screen.getAllByRole("button", { name: /show usage/ })).toHaveLength(1);
  });
});

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

describe("TabBar waiting dot", () => {
  it("a waiting tab gets the waiting class and says Claude finished its turn", () => {
    render(
      <TabBar
        tabs={[tab("a", "Claude · a")]}
        activeTabId="a"
        exited={new Set()}
        activity={{ a: "waiting" }}
        onSelect={vi.fn()}
        onClose={vi.fn()}
        onRename={vi.fn()}
        onNew={vi.fn()}
      />,
    );
    const dot = screen.getByLabelText("Claude waiting");
    expect(dot.className).toContain("tab-activity-waiting");
    expect(dot.getAttribute("title")).toBe("Claude finished its turn");
  });
});

describe("TabBar drag to reorder", () => {
  afterEach(() => vi.restoreAllMocks());

  /** Three 100px tabs a, b, c side by side. */
  function dragBar() {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const tabs = Array.from(document.querySelectorAll(".tab"));
      const at = tabs.indexOf(this);
      const left = at < 0 ? 0 : at * 100;
      return { left, right: left + 100, top: 0, bottom: 30, width: 100, height: 30, x: left, y: 0 } as DOMRect;
    });
    const props = {
      tabs: [tab("a", "A"), tab("b", "B"), tab("c", "C")],
      activeTabId: "a",
      exited: new Set<string>(),
      onSelect: vi.fn(),
      onClose: vi.fn(),
      onRename: vi.fn(),
      onNew: vi.fn(),
      onMove: vi.fn(),
    };
    render(<TabBar {...props} />);
    return props;
  }

  it("moves past a neighbour's midpoint, shows the marker, and drops at the new index", () => {
    const props = dragBar();
    const first = screen.getAllByRole("tab")[0];
    fireEvent.pointerDown(first, { clientX: 50, button: 0 });
    // Past b's middle (150) but not c's (250): lands at index 1.
    fireEvent.pointerMove(window, { clientX: 160 });
    expect(first.closest(".tab")!.className).toContain("tab-dragging");
    expect(screen.getByTestId("tab-insert-marker").style.left).toBe("200px");
    fireEvent.pointerMove(window, { clientX: 140 });
    expect(screen.getByTestId("tab-insert-marker").style.left).toBe("100px");
    fireEvent.pointerMove(window, { clientX: 290 });
    fireEvent.pointerUp(window);
    expect(props.onMove).toHaveBeenCalledWith("a", 2);
    expect(screen.queryByTestId("tab-insert-marker")).toBeNull();
    // The click that ends a drag selects nothing.
    fireEvent.click(first);
    expect(props.onSelect).not.toHaveBeenCalled();
  });

  it("Escape cancels the drag", () => {
    const props = dragBar();
    const second = screen.getAllByRole("tab")[1];
    fireEvent.pointerDown(second, { clientX: 150, button: 0 });
    fireEvent.pointerMove(window, { clientX: 20 });
    expect(screen.getByTestId("tab-insert-marker")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByTestId("tab-insert-marker")).toBeNull();
    fireEvent.pointerUp(window);
    expect(props.onMove).not.toHaveBeenCalled();
  });

  it("a click without enough movement still selects, and a drop in place moves nothing", () => {
    const props = dragBar();
    const second = screen.getAllByRole("tab")[1];
    fireEvent.pointerDown(second, { clientX: 150, button: 0 });
    fireEvent.pointerMove(window, { clientX: 152 });
    fireEvent.pointerUp(window);
    fireEvent.click(second);
    expect(props.onSelect).toHaveBeenCalledWith("b");
    fireEvent.pointerDown(second, { clientX: 150, button: 0 });
    fireEvent.pointerMove(window, { clientX: 160 });
    fireEvent.pointerUp(window);
    expect(props.onMove).not.toHaveBeenCalled();
  });

  it("the pure helpers count tabs left of the pointer and place the marker", () => {
    expect(dropIndex([50, 150], 10)).toBe(0);
    expect(dropIndex([50, 150], 100)).toBe(1);
    expect(dropIndex([50, 150], 300)).toBe(2);
    const boxes = [
      { left: 0, right: 100 },
      { left: 100, right: 200 },
    ];
    expect(markerX(boxes, 0)).toBe(0);
    expect(markerX(boxes, 1)).toBe(100);
    expect(markerX(boxes, 2)).toBe(200);
    expect(markerX([], 0)).toBeNull();
  });
});
