// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SettingsView } from "../bridge";

const setRunSettings = vi.fn(async (run: unknown) => run);
const setObserveSettings = vi.fn(async (observe: unknown) => observe);
const setClaudeSettings = vi.fn(async (claude: SettingsView["claude"]) => ({ ...view(), claude }));
vi.mock("../bridge", () => ({
  claudeStatus: vi.fn(async () => ({ found: true, path: "/opt/homebrew/bin/claude", version: "2.1.287", error: null })),
  claudeAccountLogins: vi.fn(async () => [
    {
      id: "default",
      name: "Personal",
      config: { path: "/h/.claude", display: "~/.claude", source: "default", exists: true },
      login: { state: "loggedIn", account: "jt@example.com", method: "claude.ai · max", apiKeyEnv: false },
    },
  ]),
  setClaudeSettings: (claude: SettingsView["claude"]) => setClaudeSettings(claude),
  setTerminalSettings: vi.fn(async (terminal: unknown) => terminal),
  setUiSettings: vi.fn(async (ui: unknown) => ui),
  setRunSettings: (run: unknown) => setRunSettings(run),
  setObserveSettings: (observe: unknown) => setObserveSettings(observe),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => null) }));

const { SettingsPage, loginText } = await import("./SettingsPage");

function view(): SettingsView {
  return {
    terminal: { shell: "", fontSize: 14, optionAsMeta: true, copyOnSelect: false },
    ui: { theme: "github-dark" },
    claude: { claudePath: null, accounts: [{ id: "default", name: "Personal", configDir: null }] },
    accounts: [
      {
        id: "default",
        name: "Personal",
        config: { path: "/h/.claude", display: "~/.claude", source: "default", exists: true },
        envOverride: false,
      },
    ],
    configDirEnv: null,
  };
}

describe("SettingsPage activity", () => {
  it("the all-tabs hook switch is on by default and saves when turned off", async () => {
    const onSaved = vi.fn();
    render(<SettingsPage view={view()} onSaved={onSaved} onClose={() => {}} />);
    const box = screen.getByRole("checkbox", { name: /Track activity in every Claude tab/ }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    await waitFor(() => expect(setObserveSettings).toHaveBeenCalledWith({ hooksAllTabs: false }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
  });
});

describe("SettingsPage", () => {
  it("adds an account with a safe id and saves the list", async () => {
    const onSaved = vi.fn();
    render(<SettingsPage view={view()} onSaved={onSaved} onClose={() => {}} />);
    expect(await screen.findByTestId("claude-status")).toBeTruthy();
    fireEvent.change(screen.getByRole("textbox", { name: "New account name" }), { target: { value: "Work" } });
    fireEvent.change(screen.getByRole("textbox", { name: "New account config folder" }), {
      target: { value: "~/.claude-account2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    fireEvent.click(screen.getByRole("button", { name: "Save accounts" }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(setClaudeSettings).toHaveBeenCalledWith({
      claudePath: null,
      accounts: [
        { id: "default", name: "Personal", configDir: null },
        { id: "work", name: "Work", configDir: "~/.claude-account2" },
      ],
    });
  });

  it("keeps the last account and shows each account's login", async () => {
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={() => {}} />);
    expect((screen.getByRole("button", { name: "Remove" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Check logins" }));
    expect(await screen.findByText("Signed in: jt@example.com · claude.ai · max")).toBeTruthy();
  });

  it("cleans up merged runs automatically by default, and the switch is saved", async () => {
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={() => {}} />);
    const box = screen.getByRole("checkbox", { name: /Clean up merged runs automatically/ }) as HTMLInputElement;
    expect(box.checked).toBe(true);
    fireEvent.click(box);
    fireEvent.click(screen.getByRole("button", { name: "Save run settings" }));
    await waitFor(() => expect(setRunSettings).toHaveBeenCalled());
    expect(setRunSettings.mock.calls[0][0]).toMatchObject({ autoCleanup: false });
  });

  it("closes on Escape", () => {
    const onClose = vi.fn();
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Settings" }), { key: "Escape" });
    expect(onClose).toHaveBeenCalledOnce();
  });

  it("says what each login state means", () => {
    const base = { apiKeyEnv: false };
    expect(loginText({ ...base, state: "loggedIn" })).toBe("Signed in");
    expect(loginText({ ...base, state: "loggedOut" })).toMatch(/\/login/);
    expect(loginText({ ...base, state: "noCli" })).toMatch(/not found/);
    expect(loginText({ ...base, state: "noConfigDir", detail: "Claude config folder not found: /x" })).toBe(
      "Claude config folder not found: /x",
    );
    expect(loginText({ ...base, state: "unknown", detail: "timeout" })).toBe("Unknown: timeout");
  });
});
