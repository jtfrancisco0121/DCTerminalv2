// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { SettingsView } from "../bridge";

const setRunSettings = vi.fn(async (run: unknown) => run);
const setObserveSettings = vi.fn(async (observe: unknown) => observe);
const setNotifySettings = vi.fn(async (notify: unknown) => notify);
const sendTestNotification = vi.fn(async () => {});
const setPushSettings = vi.fn(async (push: unknown) => push);
const pushStatus = vi.fn(async () => ({ saved: [] as string[] }));
const pushSetSecret = vi.fn(async (_name: string, _value: string) => ({ saved: ["ntfy-topic"] }));
const pushSendTest = vi.fn(async () => {});
const notificationPermission = vi.fn(async () => "granted");
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
  setNotifySettings: (notify: unknown) => setNotifySettings(notify),
  sendTestNotification: () => sendTestNotification(),
  notificationPermission: () => notificationPermission(),
  setPushSettings: (push: unknown) => setPushSettings(push),
  pushStatus: () => pushStatus(),
  pushSetSecret: (name: string, value: string) => pushSetSecret(name, value),
  pushSendTest: () => pushSendTest(),
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

describe("SettingsPage editor", () => {
  it("saves the editor command with the rest of the UI settings", async () => {
    const setUi = (await import("../bridge")).setUiSettings as unknown as ReturnType<typeof vi.fn>;
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={() => {}} />);
    const input = screen.getByRole("textbox", { name: "Editor command" });
    fireEvent.change(input, { target: { value: "cursor --new-window" } });
    fireEvent.blur(input);
    await waitFor(() =>
      expect(setUi).toHaveBeenCalledWith({ theme: "github-dark", editorCommand: "cursor --new-window" }),
    );
  });
});

describe("SettingsPage phone push", () => {
  const withPush = (target: string): SettingsView => ({
    ...view(),
    push: { target, ntfyServer: "https://ntfy.sh", events: {} },
  });

  it("saves a secret to the keychain only, and shows the test result inline", async () => {
    render(<SettingsPage view={withPush("ntfy")} onSaved={() => {}} onClose={() => {}} />);
    const topic = screen.getByLabelText("ntfy topic") as HTMLInputElement;
    fireEvent.change(topic, { target: { value: "my-secret-topic" } });
    fireEvent.click(screen.getByRole("button", { name: "Save ntfy topic" }));
    await waitFor(() => expect(pushSetSecret).toHaveBeenCalledWith("ntfy-topic", "my-secret-topic"));
    await waitFor(() => expect((screen.getByLabelText("ntfy topic") as HTMLInputElement).value).toBe(""));
    expect(setPushSettings).not.toHaveBeenCalledWith(expect.objectContaining({ ntfyTopic: expect.anything() }));
    fireEvent.click(screen.getByRole("button", { name: "Send test" }));
    expect(await screen.findByText("Test push sent.")).toBeTruthy();
    pushSendTest.mockRejectedValueOnce(new Error("the server answered 500"));
    fireEvent.click(screen.getByRole("button", { name: "Send test" }));
    expect(await screen.findByText("the server answered 500")).toBeTruthy();
  });

  it("offers one switch per app-only event and no push controls while off", async () => {
    const { unmount } = render(<SettingsPage view={withPush("none")} onSaved={() => {}} onClose={() => {}} />);
    expect(screen.queryByRole("button", { name: "Send test" })).toBeNull();
    unmount();
    render(<SettingsPage view={withPush("webhook")} onSaved={() => {}} onClose={() => {}} />);
    fireEvent.click(screen.getAllByRole("checkbox", { name: /A PR's check failed/ })[1]);
    await waitFor(() =>
      expect(setPushSettings).toHaveBeenCalledWith({
        target: "webhook",
        ntfyServer: "https://ntfy.sh",
        events: { "check-failed": false },
      }),
    );
  });
});

describe("SettingsPage notifications", () => {
  it("saves the master switch and one event switch, and the test button sends a test", async () => {
    const onSaved = vi.fn();
    render(<SettingsPage view={view()} onSaved={onSaved} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("checkbox", { name: /A run has made no progress/ }));
    await waitFor(() =>
      expect(setNotifySettings).toHaveBeenCalledWith({ master: true, events: { stall: false } }),
    );
    fireEvent.click(screen.getByRole("checkbox", { name: /Notify me/ }));
    await waitFor(() => expect(setNotifySettings).toHaveBeenCalledWith({ master: false, events: {} }));
    fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
    await waitFor(() => expect(sendTestNotification).toHaveBeenCalled());
    expect(screen.queryByText(/macOS blocks notifications/)).toBeNull();
  });

  it("says macOS blocks notifications when the permission is denied or the test is refused", async () => {
    notificationPermission.mockResolvedValueOnce("denied");
    const { unmount } = render(<SettingsPage view={view()} onSaved={() => {}} onClose={() => {}} />);
    expect(await screen.findByText(/macOS blocks notifications for DCTerminalv2/)).toBeTruthy();
    unmount();
    sendTestNotification.mockRejectedValueOnce(new Error("refused"));
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={() => {}} />);
    expect(screen.queryByText(/macOS blocks notifications/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Send a test notification" }));
    expect(await screen.findByText(/macOS blocks notifications for DCTerminalv2/)).toBeTruthy();
  });
});

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

describe("SettingsPage unsaved changes", () => {
  it("marks each section exactly when it differs from the saved values", async () => {
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={() => {}} />);
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    fireEvent.change(screen.getByLabelText("Account name"), { target: { value: "Work" } });
    const accounts = screen.getByRole("region", { name: "Claude accounts" });
    expect(accounts.textContent).toContain("Unsaved changes");
    expect(screen.getByRole("region", { name: "Terminal" }).textContent).not.toContain("Unsaved changes");
    fireEvent.change(screen.getByLabelText("Account name"), { target: { value: "Personal" } });
    expect(screen.queryByText("Unsaved changes")).toBeNull();

    fireEvent.change(screen.getByRole("spinbutton", { name: /Font size/ }), { target: { value: "18" } });
    expect(screen.getByRole("region", { name: "Terminal" }).textContent).toContain("Unsaved changes");
    fireEvent.click(screen.getByRole("button", { name: "Save terminal settings" }));
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).toBeNull());

    fireEvent.click(screen.getByRole("checkbox", { name: /stop hooks/ }));
    expect(screen.getByRole("region", { name: "Runs" }).textContent).toContain("Unsaved changes");
  });

  it("closes directly without changes; with changes asks, Keep editing stays, Discard closes", () => {
    const onClose = vi.fn();
    render(<SettingsPage view={view()} onSaved={() => {}} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("dialog", { name: "Settings" }), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.change(screen.getByLabelText("Account name"), { target: { value: "Work" } });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    const ask = screen.getByRole("dialog", { name: "Discard unsaved changes?" });
    expect(ask.textContent).toContain("Accounts has unsaved changes.");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.activeElement?.textContent).toBe("Keep editing");
    fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
    expect(screen.queryByRole("dialog", { name: "Discard unsaved changes?" })).toBeNull();
    expect(onClose).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(screen.getByRole("dialog", { name: "Settings" }), { key: "Escape" });
    expect(screen.getByRole("dialog", { name: "Discard unsaved changes?" })).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
