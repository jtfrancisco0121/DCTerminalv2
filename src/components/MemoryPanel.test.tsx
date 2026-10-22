// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { memoryLine, MemoryPanelView } from "./MemoryPanel";

describe("MemoryPanelView", () => {
  it("shows present / stale / missing per repo and the two buttons", () => {
    const onOnboard = vi.fn();
    const onOpen = vi.fn();
    render(
      <MemoryPanelView
        status={{
          available: true,
          workspace: { present: false },
          repos: [
            { name: "backend", id: "github.com__o__backend", present: true, staleCount: 2 },
            { name: "web", id: "github.com__o__web", present: true, topics: [{ name: "a", stale: false }] },
            { name: "engine", id: "local__engine-1234abcd", present: false },
          ],
        }}
        onOnboard={onOnboard}
        onRefresh={() => {}}
        onOpen={onOpen}
      />,
    );
    expect(screen.getByText("Workspace: missing")).toBeTruthy();
    expect(screen.getByText(/present, 2 stale/)).toBeTruthy();
    expect(screen.getByText("engine").parentElement?.textContent).toBe("engine: missing");
    fireEvent.click(screen.getByText("Onboard"));
    expect(onOnboard).toHaveBeenCalled();
    fireEvent.click(screen.getAllByText("read")[0]);
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ name: "backend" }));
  });

  it("says when memory is not available (no memory.sh yet)", () => {
    render(
      <MemoryPanelView
        status={{ available: false, problem: "memory.sh is not in this plugin yet" }}
        onOnboard={() => {}}
        onRefresh={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(screen.getByText(/memory.sh is not in this plugin yet/)).toBeTruthy();
    expect(memoryLine({ present: true })).toBe("present");
  });
});
