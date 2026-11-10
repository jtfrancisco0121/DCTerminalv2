// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { WebQaTarget } from "../bridge";

const save = vi.fn(async (targets: WebQaTarget[]) => ({
  targets: targets.map((t, i) => ({ ...t, id: t.id || `t-${i + 1}`, charters: t.charters.map((c, n) => ({ ...c, id: c.id || `c${n + 1}` })) })),
}));
const status = vi.fn(async (targetId: string) => ({ targetId, saved: false, problem: null as string | null }));
const setPw = vi.fn(async (targetId: string, _password: string) => ({ targetId, saved: true, problem: null as string | null }));
const clearPw = vi.fn(async (targetId: string) => ({ targetId, saved: false, problem: null as string | null }));
vi.mock("../run/api", () => ({
  webqaTargetsSave: (targets: WebQaTarget[]) => save(targets),
  webqaPasswordStatus: (id: string) => status(id),
  webqaPasswordSet: (id: string, password: string) => setPw(id, password),
  webqaPasswordClear: (id: string) => clearPw(id),
}));

const { WebQaTargets } = await import("./WebQaTargets");

const staging: WebQaTarget = {
  id: "t-1",
  name: "Staging",
  allowlist: ["https://app.example.com/"],
  username: "tester",
  clickupListId: "901",
  charters: [
    { id: "c2", title: "Sign in", text: "Sign in", mayChangeData: true },
    { id: "c1", title: "Search", text: "Search a word", mayChangeData: false },
  ],
};

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("WebQaTargets", () => {
  it("shows the empty state and an Add target button", () => {
    render(<WebQaTargets targets={[]} onSaved={() => {}} />);
    expect(screen.getByText("No targets yet. Add one to test a web app from outside.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Add target" })).toBeTruthy();
  });

  it("adds a target with a charter whose data-change flag is off by default", async () => {
    const onSaved = vi.fn();
    render(<WebQaTargets targets={[]} onSaved={onSaved} />);
    fireEvent.click(screen.getByRole("button", { name: "Add target" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Target name" }), { target: { value: "Prod" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Allowed URLs" }), { target: { value: "https://app.example.com/\nhttps://login.example.com/" } });
    fireEvent.click(screen.getByRole("button", { name: "Add charter" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Charter title" }), { target: { value: "Sign in" } });
    expect((screen.getByRole("checkbox", { name: "This charter may change data" }) as HTMLInputElement).checked).toBe(false);
    // The password needs a saved target.
    expect((screen.getByLabelText("Test account password") as HTMLInputElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
    const sent = save.mock.calls[0]![0];
    expect(sent[0]).toMatchObject({
      name: "Prod",
      allowlist: ["https://app.example.com/", "https://login.example.com/"],
      charters: [{ title: "Sign in", mayChangeData: false }],
    });
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(await screen.findByText("Prod")).toBeTruthy();
    expect(screen.queryByText("No targets yet. Add one to test a web app from outside.")).toBeNull();
  });

  it("refuses an invalid URL with a message and sends nothing", async () => {
    render(<WebQaTargets targets={[]} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Add target" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Target name" }), { target: { value: "Prod" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Allowed URLs" }), { target: { value: "ftp://app.example.com/" } });
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    expect((await screen.findByRole("alert")).textContent).toContain("ftp://app.example.com/ is not a URL the test can use");
    expect(save).not.toHaveBeenCalled();
  });

  it("shows the reason when the app refuses the save", async () => {
    save.mockRejectedValueOnce("Two web QA targets are called Staging. Names must differ.");
    render(<WebQaTargets targets={[staging]} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    expect((await screen.findByRole("alert")).textContent).toContain("Names must differ");
  });

  it("edits a target in place, keeps its id and orders the charters by title", async () => {
    render(<WebQaTargets targets={[staging]} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const titles = screen.getAllByRole("textbox", { name: "Charter title" }).map((el) => (el as HTMLInputElement).value);
    expect(titles).toEqual(["Search", "Sign in"]);
    fireEvent.change(screen.getByRole("textbox", { name: "Target name" }), { target: { value: "Staging 2" } });
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(save.mock.calls[0]![0][0]).toMatchObject({ id: "t-1", name: "Staging 2", username: "tester", clickupListId: "901" });
  });

  it("asks before removing and says the charters go too", async () => {
    render(<WebQaTargets targets={[staging]} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(screen.getByText("Remove target Staging? Its saved charters go too.")).toBeTruthy();
    expect(save).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Remove target" }));
    await waitFor(() => expect(save).toHaveBeenCalledWith([]));
  });

  it("saves the password through the app, never into the target", async () => {
    render(<WebQaTargets targets={[staging]} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await waitFor(() => expect(status).toHaveBeenCalledWith("t-1"));
    expect(screen.getByText(/Kept in the macOS keychain. Use a test account, never your own./)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Test account password"), { target: { value: "s3cret" } });
    fireEvent.click(screen.getByRole("button", { name: "Save password" }));
    await waitFor(() => expect(setPw).toHaveBeenCalledWith("t-1", "s3cret"));
    expect((screen.getByLabelText("Test account password") as HTMLInputElement).value).toBe("");
    fireEvent.click(screen.getByRole("button", { name: "Save target" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(JSON.stringify(save.mock.calls[0]![0])).not.toContain("s3cret");
  });

  it("disables the password with the reason when the keychain is unavailable", async () => {
    status.mockResolvedValueOnce({ targetId: "t-1", saved: false, problem: "Could not use the system keychain: the keychain refused the request." });
    render(<WebQaTargets targets={[staging]} onSaved={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await screen.findByText(/The keychain is not available: Could not use the system keychain/);
    expect((screen.getByLabelText("Test account password") as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Save password" }) as HTMLButtonElement).disabled).toBe(true);
  });
});
