// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { UI_SCALE_EVENT } from "../uiScale";
import { dropIndex, markerX } from "../tabDrag";
import type { TabSummary } from "../bridge";
import { tabProgress } from "../run/words";
import { TabBar, tabMenuItems } from "./TabBar";

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

describe("TabBar context menu and middle-click", () => {
  function menuBar() {
    const props = {
      tabs: [tab("a", "A"), tab("b", "B"), tab("c", "C")],
      activeTabId: "a",
      exited: new Set<string>(),
      onSelect: vi.fn(),
      onClose: vi.fn(),
      onRename: vi.fn(),
      onNew: vi.fn(),
      onMove: vi.fn(),
      onDuplicate: vi.fn(),
      onReveal: vi.fn(),
      onCloseMany: vi.fn(),
    };
    render(<TabBar {...props} />);
    return props;
  }
  const open = (name: string) => fireEvent.contextMenu(screen.getByRole("tab", { name }));
  const item = (name: string) => screen.getByRole("menuitem", { name });

  it("right-click lists the actions and each one works", () => {
    const props = menuBar();
    open("B");
    expect(screen.getAllByRole("menuitem").map((el) => el.textContent)).toEqual([
      "Rename",
      "Duplicate in same folder",
      "Reveal folder",
      "Move left",
      "Move right",
      "Split right",
      "Split down",
      "Close others",
      "Close to the right",
    ]);
    fireEvent.click(item("Duplicate in same folder"));
    expect(props.onDuplicate).toHaveBeenCalledWith("b");
    open("B");
    fireEvent.click(item("Reveal folder"));
    expect(props.onReveal).toHaveBeenCalledWith("b");
    open("B");
    fireEvent.click(item("Move left"));
    expect(props.onMove).toHaveBeenLastCalledWith("b", 0);
    open("B");
    fireEvent.click(item("Move right"));
    expect(props.onMove).toHaveBeenLastCalledWith("b", 2);
    open("B");
    fireEvent.click(item("Close others"));
    expect(props.onCloseMany).toHaveBeenLastCalledWith(["a", "c"]);
    open("B");
    fireEvent.click(item("Close to the right"));
    expect(props.onCloseMany).toHaveBeenLastCalledWith(["c"]);
    open("B");
    fireEvent.click(item("Rename"));
    expect(screen.getByRole("textbox", { name: "Tab name" })).toBeTruthy();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("Move left / right are disabled at the ends, and the split items until splitting exists", () => {
    menuBar();
    open("A");
    expect((item("Move left") as HTMLButtonElement).disabled).toBe(true);
    expect((item("Move right") as HTMLButtonElement).disabled).toBe(false);
    expect((item("Split right") as HTMLButtonElement).disabled).toBe(true);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    open("C");
    expect((item("Move right") as HTMLButtonElement).disabled).toBe(true);
    expect((item("Close to the right") as HTMLButtonElement).disabled).toBe(true);
  });

  it("opens from the keyboard with Shift+F10 or the context-menu key", () => {
    menuBar();
    fireEvent.keyDown(screen.getByRole("tab", { name: "B" }), { key: "F10", shiftKey: true });
    expect(screen.getByRole("menu")).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    fireEvent.keyDown(screen.getByRole("tab", { name: "C" }), { key: "ContextMenu" });
    expect(screen.getByRole("menu")).toBeTruthy();
  });

  it("middle-click closes the tab through the same close handler as the × button", () => {
    const props = menuBar();
    fireEvent(screen.getByRole("tab", { name: "B" }), new MouseEvent("auxclick", { bubbles: true, button: 1 }));
    expect(props.onClose).toHaveBeenCalledWith("b");
    fireEvent(screen.getByRole("tab", { name: "C" }), new MouseEvent("auxclick", { bubbles: true, button: 2 }));
    expect(props.onClose).toHaveBeenCalledTimes(1);
  });

  it("a lone tab has nothing to close or move", () => {
    const items = tabMenuItems([tab("a", "A")], "a", { onRename: () => {}, onMove: () => {}, onCloseMany: () => {} });
    const by = (id: string) => items.find((i) => i.id === id)!;
    expect(by("close-others").disabled).toBe(true);
    expect(by("close-right").disabled).toBe(true);
    expect(by("move-left").disabled && by("move-right").disabled).toBe(true);
  });
});

describe("TabBar split tabs", () => {
  const split = (id: string): TabSummary => ({
    ...tab(id, id.toUpperCase()),
    split: { direction: "right", ratio: 0.5, id: `${id}:p2`, cwd: `/w/${id}`, folderStatus: "ok" },
  });

  it("a split tab's dot shows its most urgent pane", () => {
    render(
      <TabBar
        tabs={[split("a")]}
        activeTabId="a"
        exited={new Set()}
        activity={{ a: "waiting", "a:p2": "needs-you" }}
        onSelect={vi.fn()}
        onClose={vi.fn()}
        onRename={vi.fn()}
        onNew={vi.fn()}
      />,
    );
    expect(screen.getByLabelText("Claude needs-you").className).toContain("tab-activity-needs-you");
  });

  it("the menu splits an unsplit tab and disables Split on one that already has two panes", () => {
    const onSplit = vi.fn();
    const props = {
      activeTabId: "a",
      exited: new Set<string>(),
      onSelect: vi.fn(),
      onClose: vi.fn(),
      onRename: vi.fn(),
      onNew: vi.fn(),
      onSplit,
    };
    const { unmount } = render(<TabBar tabs={[tab("a", "A"), split("b")]} {...props} />);
    fireEvent.contextMenu(screen.getByRole("tab", { name: "A" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Split right" }));
    expect(onSplit).toHaveBeenLastCalledWith("a", "right");
    fireEvent.contextMenu(screen.getByRole("tab", { name: "A" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Split down" }));
    expect(onSplit).toHaveBeenLastCalledWith("a", "down");
    fireEvent.contextMenu(screen.getByRole("tab", { name: "B" }));
    expect((screen.getByRole("menuitem", { name: "Split right" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("menuitem", { name: "Split down" }) as HTMLButtonElement).disabled).toBe(true);
    unmount();
  });
});

describe("TabBar run progress", () => {
  const props = {
    activeTabId: "plain",
    exited: new Set<string>(),
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onRename: vi.fn(),
    onNew: vi.fn(),
  };
  const tabs = [tab("plain", "Claude · plain"), { ...tab("r", "Eagle-Eye · r"), role: "eagle-eye", runDir: "/d/r" }];

  afterEach(cleanup);

  it("draws a two-thirds underline and puts the PRs in the tooltip and the name", () => {
    const progress = { r: tabProgress({ runId: "r-1", stage: "building", waitingFor: null, prsDone: 2, prsTotal: 3 })! };
    render(<TabBar {...props} tabs={tabs} progress={progress} />);
    const line = screen.getByTestId("tab-progress-r");
    expect(line.style.width).toBe("67%");
    expect(line.className).toContain("tone-running");
    expect(screen.getByRole("tab", { name: "Eagle-Eye · r, Building" })).toBeTruthy();
    expect(line.parentElement?.getAttribute("title")).toContain("2/3 PRs");
    expect(line.parentElement?.className).not.toContain("tab-waiting");
  });

  it("outlines a waiting run, shows a finished run full in success and a failed run in danger", () => {
    const waiting = tabProgress({ runId: "r-1", stage: "plan-approval", waitingFor: "plan-approval", prsDone: 0, prsTotal: 0 })!;
    const { rerender } = render(<TabBar {...props} tabs={tabs} progress={{ r: waiting }} />);
    expect(screen.getByTestId("tab-progress-r").parentElement?.className).toContain("tab-waiting");
    const done = tabProgress({ runId: "r-1", stage: "done", waitingFor: null, prsDone: 1, prsTotal: 1 })!;
    rerender(<TabBar {...props} tabs={tabs} progress={{ r: done }} />);
    expect(screen.getByTestId("tab-progress-r").style.width).toBe("100%");
    expect(screen.getByTestId("tab-progress-r").className).toContain("tone-success");
    const failed = tabProgress({ runId: "r-1", stage: "failed", waitingFor: null, prsDone: 0, prsTotal: 0 })!;
    rerender(<TabBar {...props} tabs={tabs} progress={{ r: failed }} />);
    expect(screen.getByTestId("tab-progress-r").className).toContain("tone-danger");
    expect(screen.getByRole("tab", { name: /Failed/ })).toBeTruthy();
  });

  it("shows the stage word under a run tab's name, and none on a plain tab; the word carries no drag attribute", () => {
    const waiting = tabProgress({ runId: "r-1", stage: "plan-approval", waitingFor: "plan-approval", prsDone: 0, prsTotal: 0 })!;
    const { rerender, container } = render(<TabBar {...props} tabs={tabs} progress={{ r: waiting }} />);
    const stage = container.querySelector(".tab-stage")!;
    expect(stage.textContent).toBe("Plan approval");
    expect(container.querySelectorAll(".tab-stage")).toHaveLength(1);
    expect(stage.hasAttribute("data-tauri-drag-region")).toBe(false);
    expect(screen.getByRole("tab", { name: "Eagle-Eye · r, Plan approval, waiting for you" })).toBeTruthy();
    const failed = tabProgress({ runId: "r-1", stage: "failed", waitingFor: null, prsDone: 0, prsTotal: 0 })!;
    rerender(<TabBar {...props} tabs={tabs} progress={{ r: failed }} />);
    expect(container.querySelector(".tab-stage")!.textContent).toBe("Failed");
    expect(screen.getByRole("tab", { name: "Claude · plain" }).querySelector(".tab-stage")).toBeNull();
  });

  it("renders a tab without a run as before", () => {
    render(<TabBar {...props} tabs={tabs} progress={{}} />);
    expect(screen.queryByTestId("tab-progress-plain")).toBeNull();
    expect(screen.getByRole("tab", { name: "Claude · plain" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Claude · plain" }).parentElement?.getAttribute("title")).toBe("/w/plain");
  });
});

describe("TabBar overflow", () => {
  const props = {
    exited: new Set<string>(),
    onSelect: vi.fn(),
    onClose: vi.fn(),
    onRename: vi.fn(),
    onNew: vi.fn(),
  };
  const tabs = [tab("a", "Claude · a"), tab("b", "Claude · b"), { ...tab("r", "Eagle-Eye · r"), role: "eagle-eye", runDir: "/d/r" }, tab("d", "Claude · d")];
  let rects: Record<string, { left: number; right: number }>;
  let observers: Array<() => void>;
  let scrolled: string[];
  const original = {
    rect: HTMLElement.prototype.getBoundingClientRect,
    scroll: HTMLElement.prototype.scrollIntoView,
    observer: globalThis.ResizeObserver,
  };

  beforeEach(() => {
    rects = {};
    observers = [];
    scrolled = [];
    HTMLElement.prototype.getBoundingClientRect = function () {
      const text = this.classList.contains("tab") ? (this.querySelector("[role=tab]")?.textContent ?? "").replace(/^✳/, "") : "";
      const key = this.classList.contains("tab-bar") ? "bar" : Object.keys(rects).find((k) => k !== "bar" && text.startsWith(k));
      const r = (key && rects[key]) || { left: 0, right: 0 };
      return { ...r, top: 0, bottom: 20, width: r.right - r.left, height: 20, x: r.left, y: 0, toJSON: () => ({}) };
    };
    HTMLElement.prototype.scrollIntoView = function () {
      scrolled.push(this.querySelector("[role=tab]")?.textContent ?? "?");
    };
    globalThis.ResizeObserver = class {
      constructor(cb: () => void) {
        observers.push(cb);
      }
      observe() {}
      disconnect() {}
    } as never;
  });
  afterEach(() => {
    cleanup();
    HTMLElement.prototype.getBoundingClientRect = original.rect;
    HTMLElement.prototype.scrollIntoView = original.scroll;
    globalThis.ResizeObserver = original.observer;
  });

  const fit = () => {
    rects = { bar: { left: 0, right: 250 } };
    for (const [i, t] of tabs.entries()) rects[t.label] = { left: i * 100, right: i * 100 + 100 };
  };
  const word = tabProgress({ runId: "r-1", stage: "building", waitingFor: null, prsDone: 0, prsTotal: 0 })!;

  it("shows no +N when every tab is whole", () => {
    rects = { bar: { left: 0, right: 1000 } };
    for (const [i, t] of tabs.entries()) rects[t.label] = { left: i * 100, right: i * 100 + 100 };
    render(<TabBar {...props} tabs={tabs} activeTabId="a" />);
    expect(screen.queryByTitle(/more tab/)).toBeNull();
  });

  it("shows the count of hidden tabs and lists exactly those, with the stage word; choosing selects only", () => {
    fit();
    const onSelect = vi.fn();
    render(<TabBar {...props} onSelect={onSelect} tabs={tabs} activeTabId="a" progress={{ r: word }} />);
    const more = screen.getByRole("button", { name: "+2" });
    expect(more.getAttribute("title")).toBe("2 more tabs");
    expect(more.getAttribute("aria-haspopup")).toBe("menu");
    expect(more.hasAttribute("data-tauri-drag-region")).toBe(false);
    expect(more.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(more);
    expect(more.getAttribute("aria-expanded")).toBe("true");
    const items = screen.getAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual(["Eagle-Eye · rBuilding", "Claude · d"]);
    fireEvent.click(items[1]);
    expect(onSelect).toHaveBeenCalledExactlyOnceWith("d");
    expect(props.onClose).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("closes the menu with Escape and gives the focus back to +N", () => {
    fit();
    render(<TabBar {...props} tabs={tabs} activeTabId="a" />);
    const more = screen.getByRole("button", { name: "+2" });
    fireEvent.click(more);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(more);
  });

  it("scrolls the active tab into view when activeTabId changes, and only then", () => {
    fit();
    const { rerender } = render(<TabBar {...props} tabs={tabs} activeTabId="a" />);
    scrolled = [];
    rerender(<TabBar {...props} tabs={tabs} activeTabId="d" progress={{ r: word }} />);
    expect(scrolled).toEqual(["✳Claude · d"]);
    scrolled = [];
    rerender(<TabBar {...props} tabs={tabs} activeTabId="d" />);
    fireEvent.scroll(document.querySelector(".tab-bar")!);
    expect(scrolled).toEqual([]);
  });

  it("measures again on a UI scale change and when the row resizes", () => {
    rects = { bar: { left: 0, right: 1000 } };
    for (const [i, t] of tabs.entries()) rects[t.label] = { left: i * 100, right: i * 100 + 100 };
    render(<TabBar {...props} tabs={tabs} activeTabId="a" />);
    expect(screen.queryByTitle(/more tab/)).toBeNull();
    rects.bar = { left: 0, right: 250 };
    act(() => {
      window.dispatchEvent(new CustomEvent(UI_SCALE_EVENT, { detail: 125 }));
    });
    expect(screen.getByRole("button", { name: "+2" })).toBeTruthy();
    rects.bar = { left: 0, right: 1000 };
    act(() => observers.forEach((cb) => cb()));
    expect(screen.queryByTitle(/more tab/)).toBeNull();
  });

  it("a tab chosen from +N is scrolled into view and +N stops counting it, with no scroll event", () => {
    // QA S5.1 at 420px: the chosen tab stayed cut and "+1" stayed. The row is 150 wide and shows a and half of b.
    rects = { bar: { left: 0, right: 150 }, "Claude · a": { left: 0, right: 100 }, "Claude · b": { left: 100, right: 200 } };
    const two = tabs.slice(0, 2);
    // Scrolling b into view moves the row by 50 (the scroll event a browser sends later is not sent here).
    HTMLElement.prototype.scrollIntoView = function () {
      const text = this.querySelector("[role=tab]")?.textContent ?? "?";
      scrolled.push(text);
      if (!text.endsWith("b")) return;
      rects["Claude · a"] = { left: -50, right: 50 };
      rects["Claude · b"] = { left: 50, right: 150 };
    };
    let active = "a";
    const onSelect = vi.fn((id: string) => (active = id));
    const { rerender } = render(<TabBar {...props} onSelect={onSelect} tabs={two} activeTabId={active} />);
    fireEvent.click(screen.getByRole("button", { name: "+1" }));
    const items = screen.getAllByRole("menuitem");
    expect(items.map((item) => item.textContent)).toEqual(["Claude · b"]);
    fireEvent.click(items[0]);
    expect(onSelect).toHaveBeenCalledExactlyOnceWith("b");
    rerender(<TabBar {...props} onSelect={onSelect} tabs={two} activeTabId={active} />);
    expect(scrolled[scrolled.length - 1]).toBe("✳Claude · b");
    // Now a is the cut one: "+1" lists a, not b.
    fireEvent.click(screen.getByRole("button", { name: "+1" }));
    expect(screen.getAllByRole("menuitem").map((item) => item.textContent)).toEqual(["Claude · a"]);
  });

  it("publishes the row's measured width as --tab-view, which caps every tab (QA S5.1 at 420px and 320px)", () => {
    rects = { bar: { left: 0, right: 150.6 }, "Claude · a": { left: 0, right: 100 }, "Claude · b": { left: 100, right: 200 } };
    render(<TabBar {...props} tabs={tabs.slice(0, 2)} activeTabId="a" />);
    const row = screen.getByRole("tablist");
    expect(row.style.getPropertyValue("--tab-view")).toBe("150px");
    // Less the side padding, so a capped tab fits inside the padding too.
    row.style.paddingLeft = "6px";
    row.style.paddingRight = "6px";
    act(() => observers.forEach((cb) => cb()));
    expect(row.style.getPropertyValue("--tab-view")).toBe("138px");
  });
});
