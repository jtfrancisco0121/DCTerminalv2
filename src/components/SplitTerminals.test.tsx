// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { TabSummary } from "../bridge";

vi.mock("./TerminalView", () => ({
  TerminalView: ({ tab, autoFocus }: { tab: TabSummary; autoFocus?: boolean }) => (
    <div data-testid={`terminal-${tab.id}`} data-kind={tab.kind} data-cwd={tab.cwd} data-autofocus={String(autoFocus)}>
      {tab.label}
    </div>
  ),
}));

const { clampRatio, paneTab, SplitTerminals } = await import("./SplitTerminals");

const tab = (direction: "right" | "down" = "right", ratio = 0.5): TabSummary => ({
  id: "t1",
  label: "Claude · app",
  customLabel: false,
  cwd: "/w/app",
  kind: "claude",
  folderStatus: "ok",
  hasSession: true,
  split: { direction, ratio, id: "t1:p2", cwd: "/w/app", folderStatus: "ok" },
});

function view(over: Partial<React.ComponentProps<typeof SplitTerminals>> = {}) {
  const props = {
    tab: tab(),
    fontSize: 14,
    autoFocus: true,
    focusedPane: 1 as const,
    onPaneFocus: vi.fn(),
    onFolderChanged: vi.fn(),
    onRatio: vi.fn(),
    ...over,
  };
  render(<SplitTerminals {...props} />);
  return props;
}

describe("SplitTerminals", () => {
  afterEach(cleanup);

  it("shows the tab's own terminal and a Shell second pane with its own id and folder", () => {
    view();
    expect(screen.getByTestId("terminal-t1").getAttribute("data-kind")).toBe("claude");
    const second = screen.getByTestId("terminal-t1:p2");
    expect(second.getAttribute("data-kind")).toBe("shell");
    expect(second.getAttribute("data-cwd")).toBe("/w/app");
    expect(paneTab(tab()).id).toBe("t1:p2");
    expect(paneTab(tab()).split).toBeNull();
  });

  it("lays the panes out right or down and takes keyboard focus in the focused pane only", () => {
    const { unmount } = render(
      <SplitTerminals
        tab={tab("down")}
        fontSize={14}
        autoFocus
        focusedPane={2}
        onPaneFocus={() => {}}
        onFolderChanged={() => {}}
        onRatio={() => {}}
      />,
    );
    expect(screen.getByTestId("split").className).toContain("split-down");
    expect(screen.getByRole("separator").getAttribute("aria-orientation")).toBe("horizontal");
    expect(screen.getByTestId("terminal-t1").getAttribute("data-autofocus")).toBe("false");
    expect(screen.getByTestId("terminal-t1:p2").getAttribute("data-autofocus")).toBe("true");
    unmount();
    view();
    expect(screen.getByTestId("split").className).toContain("split-right");
    expect(screen.getByRole("separator").getAttribute("aria-orientation")).toBe("vertical");
  });

  it("reports which pane the keyboard is in", () => {
    const props = view();
    fireEvent.focus(screen.getByTestId("pane-2"));
    expect(props.onPaneFocus).toHaveBeenLastCalledWith(2);
    fireEvent.mouseDown(screen.getByTestId("pane-1"));
    expect(props.onPaneFocus).toHaveBeenLastCalledWith(1);
  });

  it("the divider moves by arrow keys, clamped to 0.2..0.8, and resets on double-click", () => {
    const props = view({ tab: tab("right", 0.5) });
    const divider = screen.getByRole("separator");
    expect(divider.getAttribute("aria-valuenow")).toBe("50");
    fireEvent.keyDown(divider, { key: "ArrowRight" });
    expect(props.onRatio).toHaveBeenLastCalledWith(0.55);
    fireEvent.keyDown(divider, { key: "ArrowLeft" });
    expect(props.onRatio).toHaveBeenLastCalledWith(0.45);
    fireEvent.keyDown(divider, { key: "ArrowDown" });
    expect(props.onRatio).toHaveBeenCalledTimes(2);
    fireEvent.doubleClick(divider);
    expect(props.onRatio).toHaveBeenLastCalledWith(0.5);
    expect(clampRatio(0.01)).toBe(0.2);
    expect(clampRatio(2)).toBe(0.8);
    expect(clampRatio(Number.NaN)).toBe(0.5);
  });

  it("dragging the divider resizes live and saves the share on release", () => {
    const props = view();
    const box = screen.getByTestId("split");
    vi.spyOn(box, "getBoundingClientRect").mockReturnValue({ left: 0, top: 0, width: 1000, height: 400 } as DOMRect);
    const divider = screen.getByRole("separator");
    fireEvent.pointerDown(divider, { pointerId: 1, clientX: 500 });
    fireEvent.pointerMove(divider, { clientX: 300 });
    expect(divider.getAttribute("aria-valuenow")).toBe("30");
    fireEvent.pointerMove(divider, { clientX: 20 });
    expect(divider.getAttribute("aria-valuenow")).toBe("20");
    fireEvent.pointerUp(divider);
    expect(props.onRatio).toHaveBeenCalledWith(0.2);
  });
});
