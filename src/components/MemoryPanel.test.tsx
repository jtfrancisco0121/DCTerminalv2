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

  it("shows the Git Bash problem as the backend wrote it", () => {
    const text = "Git Bash not found. Install Git for Windows or set CLAUDE_CODE_GIT_BASH_PATH.";
    render(
      <MemoryPanelView
        status={{ available: false, problem: text }}
        onOnboard={() => {}}
        onRefresh={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(screen.getByText(`Not available: ${text}`)).toBeTruthy();
  });

  const repo = {
    name: "backend",
    id: "github.com__o__backend",
    present: true,
    topics: [
      { name: "api", stale: true, checkedAt: "abc123" },
      { name: "db-schema", stale: false },
      { name: "../evil", stale: false },
    ],
  };

  it("lists topics with stale badges and opens a valid one; invalid names are not clickable", () => {
    const onOpenTopic = vi.fn();
    render(
      <MemoryPanelView
        status={{ available: true, repos: [repo] }}
        onOnboard={() => {}}
        onRefresh={() => {}}
        onOpen={() => {}}
        onOpenTopic={onOpenTopic}
      />,
    );
    // Stale topics: expanded by default.
    expect(screen.getByText("api").closest("li")?.textContent).toContain("stale");
    expect(screen.getByText("api").closest("li")?.querySelector(".stale-badge")?.getAttribute("title")).toBe("checked at abc123");
    expect(screen.getByText("db-schema").closest("li")?.querySelector(".stale-badge")).toBeNull();
    fireEvent.click(screen.getByText("db-schema"));
    expect(onOpenTopic).toHaveBeenCalledWith(expect.objectContaining({ id: "github.com__o__backend" }), "db-schema");
    expect(screen.getByText("../evil").tagName).toBe("SPAN");
  });

  it("is collapsed when nothing is stale, and says when there are no topics", () => {
    render(
      <MemoryPanelView
        status={{ available: true, repos: [{ ...repo, topics: [{ name: "db-schema", stale: false }] }, { name: "web", id: "w", present: true, topics: [] }] }}
        onOnboard={() => {}}
        onRefresh={() => {}}
        onOpen={() => {}}
      />,
    );
    expect(screen.queryByText("db-schema")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Show topics of backend" }));
    expect(screen.getByText("db-schema")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Show topics of web" }));
    expect(screen.getByText("No topics yet.")).toBeTruthy();
  });

  it("offers Refresh stale only with stale topics and passes the repo", () => {
    const onRefreshStale = vi.fn();
    const { rerender } = render(
      <MemoryPanelView
        status={{ available: true, repos: [repo, { name: "web", id: "w", present: true, topics: [{ name: "a", stale: false }] }] }}
        onOnboard={() => {}}
        onRefresh={() => {}}
        onOpen={() => {}}
        onRefreshStale={onRefreshStale}
      />,
    );
    const buttons = screen.getAllByRole("button", { name: /Refresh stale/ });
    expect(buttons).toHaveLength(1);
    expect(buttons[0].textContent).toBe("Refresh stale (1)");
    fireEvent.click(buttons[0]);
    expect(onRefreshStale).toHaveBeenCalledWith(expect.objectContaining({ name: "backend" }));
    rerender(
      <MemoryPanelView
        status={{ available: true, repos: [repo] }}
        onOnboard={() => {}}
        onRefresh={() => {}}
        onOpen={() => {}}
        onRefreshStale={onRefreshStale}
        busy
      />,
    );
    expect((screen.getByRole("button", { name: /Refresh stale/ }) as HTMLButtonElement).disabled).toBe(true);
  });
});
