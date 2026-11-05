// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExportPreview } from "../flight/types";

const flightExportPreview = vi.fn();
const flightExportSend = vi.fn();
const flightExportSave = vi.fn();
vi.mock("../flight/api", () => ({
  flightExportPreview: (...args: unknown[]) => flightExportPreview(...args),
  flightExportSend: (...args: unknown[]) => flightExportSend(...args),
  flightExportSave: (...args: unknown[]) => flightExportSave(...args),
}));
const save = vi.fn();
vi.mock("@tauri-apps/plugin-dialog", () => ({ save: (...args: unknown[]) => save(...args) }));

const { ExportDialog } = await import("./ExportDialog");

const preview = (over: Partial<ExportPreview> = {}): ExportPreview => ({
  policy: "metadata",
  encoding: "protobuf",
  endpoint: "http://127.0.0.1:6006/v1/traces",
  spans: 12,
  events: 3,
  bytes: 2048,
  digest: "d1",
  fields: [{ field: "gen_ai.operation.name", count: 9, leaves: 9 }],
  warnings: [],
  loopback: true,
  capturePolicy: "metadata",
  ...over,
});

const target = { runDir: "/runs/r-1" };

beforeEach(() => {
  flightExportPreview.mockReset().mockResolvedValue(preview());
  flightExportSend.mockReset().mockResolvedValue({ endpoint: "http://127.0.0.1:6006/v1/traces", spans: 12, status: 200 });
  flightExportSave.mockReset().mockResolvedValue(undefined);
  save.mockReset().mockResolvedValue("/tmp/r-1.otlp.pb");
});
afterEach(cleanup);

describe("ExportDialog", () => {
  it("opens on metadata and protobuf, keeps Send disabled while the preview loads, then shows the counts", async () => {
    let finish: (p: ExportPreview) => void = () => {};
    flightExportPreview.mockReturnValueOnce(new Promise<ExportPreview>((r) => (finish = r)));
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    expect((screen.getByLabelText("Export policy") as HTMLSelectElement).value).toBe("metadata");
    expect((screen.getByLabelText("Export encoding") as HTMLSelectElement).value).toBe("protobuf");
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Save to file" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId("export-loading").textContent).toContain("Building the preview");
    finish(preview());
    expect((await screen.findByTestId("export-size")).textContent).toBe("12 spans, 3 events, 2.0 KiB");
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByLabelText("Export endpoint") as HTMLInputElement).value).toBe("http://127.0.0.1:6006/v1/traces");
    expect(flightExportPreview).toHaveBeenCalledWith(target, { policy: "metadata", encoding: "protobuf", confirmFull: false });
  });

  it("lists full only for a full capture policy and needs an unticked confirmation each time", async () => {
    const { unmount } = render(<ExportDialog target={target} capturePolicy="redacted" trace="r-1" onClose={() => {}} />);
    await screen.findByTestId("export-size");
    expect(screen.queryByRole("option", { name: /full/ })).toBeNull();
    unmount();
    render(<ExportDialog target={target} capturePolicy="full" trace="r-1" onClose={() => {}} />);
    await screen.findByTestId("export-size");
    fireEvent.change(screen.getByLabelText("Export policy"), { target: { value: "full" } });
    const box = (await screen.findByRole("checkbox")) as HTMLInputElement;
    expect(box.checked).toBe(false);
    await waitFor(() =>
      expect(flightExportPreview).toHaveBeenLastCalledWith(
        target,
        expect.objectContaining({ policy: "full", confirmFull: false }),
      ),
    );
    fireEvent.click(box);
    await waitFor(() =>
      expect(flightExportPreview).toHaveBeenLastCalledWith(
        target,
        expect.objectContaining({ policy: "full", confirmFull: true }),
      ),
    );
  });

  it("warns before Send when the endpoint leaves the machine", async () => {
    flightExportPreview.mockResolvedValue(preview({ endpoint: "http://10.0.0.5:6006/v1/traces", loopback: false }));
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    expect((await screen.findByTestId("export-leaves")).textContent).toBe(
      "This export leaves the machine: 12 spans to http://10.0.0.5:6006/v1/traces",
    );
  });

  it("sends the previewed digest and says what was sent", async () => {
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    await screen.findByTestId("export-size");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByTestId("export-outcome")).textContent).toBe("Sent 12 spans to http://127.0.0.1:6006/v1/traces.");
    expect(flightExportSend).toHaveBeenCalledWith("d1", "http://127.0.0.1:6006/v1/traces");
  });

  it("saves the previewed digest to the picked path", async () => {
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    await screen.findByTestId("export-size");
    fireEvent.click(screen.getByRole("button", { name: "Save to file" }));
    expect((await screen.findByTestId("export-outcome")).textContent).toBe("Saved to /tmp/r-1.otlp.pb.");
    expect(save).toHaveBeenCalledWith({ defaultPath: "r-1.otlp.pb" });
    expect(flightExportSave).toHaveBeenCalledWith("d1", "/tmp/r-1.otlp.pb");
  });

  it("says the trace changed and previews again; a failed send keeps the preview", async () => {
    flightExportSend.mockRejectedValueOnce("the trace changed since the preview");
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    await screen.findByTestId("export-size");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByTestId("export-failed")).textContent).toContain("The trace changed since the preview; preview again.");
    flightExportPreview.mockResolvedValueOnce(preview({ digest: "d2", spans: 14 }));
    fireEvent.click(screen.getByRole("button", { name: "Preview again" }));
    await waitFor(() => expect(screen.getByTestId("export-size").textContent).toContain("14 spans"));
    flightExportSend.mockRejectedValueOnce("http://127.0.0.1:6006/v1/traces answered 500 Internal Server Error");
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect((await screen.findByTestId("export-failed")).textContent).toContain("answered 500");
    expect(screen.getByTestId("export-size")).toBeTruthy();
  });

  it("shows the too-large message and keeps Send disabled", async () => {
    flightExportPreview.mockRejectedValue("This trace is larger than 32 MiB; export a single session instead.");
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    expect((await screen.findByTestId("export-too-large")).textContent).toContain("larger than 32 MiB");
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("Enter in the endpoint field rebuilds the preview and never sends", async () => {
    render(<ExportDialog target={target} capturePolicy="metadata" trace="r-1" onClose={() => {}} />);
    await screen.findByTestId("export-size");
    const field = screen.getByLabelText("Export endpoint");
    fireEvent.change(field, { target: { value: "http://localhost:4318/v1/traces" } });
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(true);
    flightExportPreview.mockResolvedValueOnce(preview({ endpoint: "http://localhost:4318/v1/traces", digest: "d3" }));
    fireEvent.keyDown(field, { key: "Enter" });
    await waitFor(() => expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false));
    expect(flightExportSend).not.toHaveBeenCalled();
    expect(flightExportPreview).toHaveBeenLastCalledWith(target, expect.objectContaining({ endpoint: "http://localhost:4318/v1/traces" }));
  });
});
