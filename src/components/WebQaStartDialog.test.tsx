// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../bridge", () => ({
  getSettings: vi.fn(async () => ({
    webQa: {
      targets: [{ id: "t-1", name: "Staging", allowlist: ["https://app.example.com/"], username: "", clickupListId: null, charters: [] }],
    },
  })),
}));
vi.mock("../run/api", () => ({ webqaStart: vi.fn() }));

const { WebQaStartDialog } = await import("./WebQaStartDialog");

afterEach(cleanup);

describe("WebQaStartDialog", () => {
  it("disables Start for a target with no charters and says why", async () => {
    render(<WebQaStartDialog windowId="w1" onClose={() => {}} onStarted={() => {}} />);
    await screen.findByLabelText("Web QA target");
    const start = screen.getByRole("button", { name: "Start pass" }) as HTMLButtonElement;
    expect(start.disabled).toBe(true);
    expect(start.title).toBe("Add a charter in Settings › Web QA");
  });
});
