// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RepoSetup } from "../run/api";

const setRepoSetup = vi.fn(async (_path: string, setup: RepoSetup) => ({ ...setup, copyFiles: setup.copyFiles }));
vi.mock("../run/api", () => ({ setRepoSetup: (path: string, setup: RepoSetup) => setRepoSetup(path, setup) }));

const { WorktreeSetup } = await import("./WorktreeSetup");

describe("WorktreeSetup receipts", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("is off by default, saves the choice and shows the saved value", async () => {
    render(<WorktreeSetup repos={[{ name: "api", path: "/w/api", check: null }]} />);
    const box = screen.getByLabelText("Receipts in PR bodies for api") as HTMLInputElement;
    expect(box.checked).toBe(false);
    fireEvent.click(box);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(setRepoSetup).toHaveBeenCalledOnce());
    expect(setRepoSetup.mock.calls[0][1]).toMatchObject({ receipt: true });
    await screen.findByText("Saved.");
    expect((screen.getByLabelText("Receipts in PR bodies for api") as HTMLInputElement).checked).toBe(true);
  });

  it("starts checked for a repo that has it on", () => {
    render(<WorktreeSetup repos={[{ name: "api", path: "/w/api", check: null, receipt: true }]} />);
    expect((screen.getByLabelText("Receipts in PR bodies for api") as HTMLInputElement).checked).toBe(true);
  });
});
