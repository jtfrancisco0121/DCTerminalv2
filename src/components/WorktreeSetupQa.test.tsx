// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { RepoSetup } from "../run/api";

const setRepoSetup = vi.fn(async (_path: string, setup: RepoSetup) => ({ ...setup }));
vi.mock("../run/api", () => ({ setRepoSetup: (path: string, setup: RepoSetup) => setRepoSetup(path, setup) }));

const { WorktreeSetup } = await import("./WorktreeSetup");

const repo = { name: "api", path: "/w/api", check: null };

describe("WorktreeSetup QA", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("hides the inputs until the box is ticked and saves null when off", async () => {
    render(<WorktreeSetup repos={[repo]} />);
    expect(screen.queryByLabelText("QA command of api")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(setRepoSetup).toHaveBeenCalledOnce());
    expect(setRepoSetup.mock.calls[0][1].qa).toBeNull();
    fireEvent.click(screen.getByLabelText("Try PRs in the app (QA) for api"));
    expect(screen.getByLabelText("QA command of api")).toBeTruthy();
    expect((screen.getByLabelText("QA URL of api") as HTMLInputElement).value).toBe("http://127.0.0.1:%PORT%/");
  });

  it("saves the command and URL", async () => {
    render(<WorktreeSetup repos={[repo]} />);
    fireEvent.click(screen.getByLabelText("Try PRs in the app (QA) for api"));
    fireEvent.change(screen.getByLabelText("QA command of api"), { target: { value: "npm run dev:fake" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(setRepoSetup).toHaveBeenCalledOnce());
    expect(setRepoSetup.mock.calls[0][1].qa).toEqual({ command: "npm run dev:fake", url: "http://127.0.0.1:%PORT%/" });
  });

  it("shows the refusal inline, linked to the inputs, and keeps the typed values", async () => {
    setRepoSetup.mockRejectedValueOnce("QA URL: use http:// with 127.0.0.1, localhost or [::1].");
    render(<WorktreeSetup repos={[repo]} />);
    fireEvent.click(screen.getByLabelText("Try PRs in the app (QA) for api"));
    fireEvent.change(screen.getByLabelText("QA command of api"), { target: { value: "npm run dev:fake" } });
    fireEvent.change(screen.getByLabelText("QA URL of api"), { target: { value: "http://localhost.evil.com/" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("QA URL");
    expect(screen.getByLabelText("QA URL of api").getAttribute("aria-describedby")).toBe(alert.id);
    expect((screen.getByLabelText("QA command of api") as HTMLInputElement).value).toBe("npm run dev:fake");
    expect((screen.getByLabelText("QA URL of api") as HTMLInputElement).value).toBe("http://localhost.evil.com/");
  });

  it("starts ticked for a repo that has QA on", () => {
    render(
      <WorktreeSetup
        repos={[{ ...repo, qa: { command: "npm run dev:fake", url: "http://127.0.0.1:%PORT%/" } }]}
      />,
    );
    expect((screen.getByLabelText("Try PRs in the app (QA) for api") as HTMLInputElement).checked).toBe(true);
    expect((screen.getByLabelText("QA command of api") as HTMLInputElement).value).toBe("npm run dev:fake");
  });
});
