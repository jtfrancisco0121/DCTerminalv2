// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const setTabFolder = vi.fn(async (_id: string, _cwd: string) => ({}));
const open = vi.fn(async (): Promise<string | null> => "/w/new");
vi.mock("../bridge", () => ({ setTabFolder: (id: string, cwd: string) => setTabFolder(id, cwd) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => open() }));

const { FolderMissing } = await import("./FolderMissing");

describe("FolderMissing", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows why and points the tab at the chosen folder", async () => {
    const onChanged = vi.fn();
    render(<FolderMissing tabId="tab_1" problem="Folder not found: /w/old" onChanged={onChanged} />);
    expect(screen.getByText("Folder not found: /w/old")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Choose another folder…" }));
    await waitFor(() => expect(onChanged).toHaveBeenCalledOnce());
    expect(setTabFolder).toHaveBeenCalledWith("tab_1", "/w/new");
  });

  it("does nothing when the picker is cancelled, and shows a refusal", async () => {
    const onChanged = vi.fn();
    open.mockResolvedValueOnce(null);
    render(<FolderMissing tabId="tab_1" problem="gone" onChanged={onChanged} />);
    fireEvent.click(screen.getByRole("button", { name: "Choose another folder…" }));
    await waitFor(() => expect(open).toHaveBeenCalled());
    expect(setTabFolder).not.toHaveBeenCalled();
    open.mockResolvedValueOnce("/w/bad");
    setTabFolder.mockRejectedValueOnce(new Error("Folder not found: /w/bad"));
    fireEvent.click(screen.getByRole("button", { name: "Choose another folder…" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Folder not found: /w/bad");
    expect(onChanged).not.toHaveBeenCalled();
  });
});
