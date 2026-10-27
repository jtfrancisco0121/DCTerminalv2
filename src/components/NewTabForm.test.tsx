// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const recentFolders = vi.fn(async (): Promise<string[]> => []);
vi.mock("../bridge", () => ({ recentFolders: () => recentFolders() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const { NewTabForm } = await import("./NewTabForm");

describe("NewTabForm recent folders", () => {
  afterEach(() => {
    cleanup();
    recentFolders.mockReset();
  });

  it("offers the recent folders as suggestions for the folder field", async () => {
    recentFolders.mockResolvedValue(["/w/app", "/w/web", "/w/api"]);
    const { container } = render(<NewTabForm initial={{ cwd: "", kind: "claude" }} onCreate={async () => {}} />);
    const input = screen.getByPlaceholderText("/path/to/project");
    const list = input.getAttribute("list");
    expect(list).toBeTruthy();
    await waitFor(() => expect(container.querySelectorAll(`datalist#${list} option`)).toHaveLength(3));
    const values = Array.from(container.querySelectorAll(`datalist#${list} option`)).map((o) => o.getAttribute("value"));
    expect(values).toEqual(["/w/app", "/w/web", "/w/api"]);
  });

  it("still works with no suggestions or when they cannot be read", async () => {
    recentFolders.mockRejectedValue(new Error("no"));
    const { container } = render(<NewTabForm initial={{ cwd: "/w/x", kind: "shell" }} onCreate={async () => {}} />);
    await waitFor(() => expect(recentFolders).toHaveBeenCalled());
    expect(container.querySelectorAll("datalist option")).toHaveLength(0);
    expect((screen.getByPlaceholderText("/path/to/project") as HTMLInputElement).value).toBe("/w/x");
  });
});
