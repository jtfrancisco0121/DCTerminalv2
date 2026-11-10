// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import fixture from "../../fixtures/run/run-view.json";
import type { PrView, ReceiptView } from "../run/api";
import { ReceiptSection } from "./ReceiptSection";

const RUN = "r-20261022-1304";
const pr = (receipt?: ReceiptView): PrView => ({ ...(fixture.prs[0] as unknown as PrView), id: "PR1", receipt });
const link = `dcterminalv2://run/${RUN}`;
const rows: ReceiptView = {
  state: "rows",
  rows: ["Agent implementer: 2 run(s), 6.5 min active", "Gate: PASS", `Trace on the machine that ran it: [${link}](${link}) (\`${link}\`)`],
};

afterEach(cleanup);

describe("the Receipt section", () => {
  it("is a closed disclosure named for its PR", () => {
    render(<ReceiptSection pr={pr(rows)} runId={RUN} />);
    const summary = screen.getByRole("group").querySelector("summary")!;
    expect(summary.textContent).toBe("Receipt");
    expect(summary.getAttribute("aria-label")).toBe("Receipt for PR1");
    expect((screen.getByRole("group") as HTMLDetailsElement).open).toBe(false);
  });

  it("lists the rows in order", () => {
    render(<ReceiptSection pr={pr(rows)} runId={RUN} />);
    const items = [...document.querySelectorAll("li")].map((li) => li.textContent);
    expect(items[0]).toBe("Agent implementer: 2 run(s), 6.5 min active");
    expect(items[1]).toBe("Gate: PASS");
    expect(items).toHaveLength(3);
  });

  it("says why there are no rows", () => {
    render(<ReceiptSection pr={pr({ state: "off", rows: [] })} runId={RUN} />);
    expect(screen.getByText("Receipts are off for this repo")).toBeTruthy();
    cleanup();
    render(<ReceiptSection pr={pr({ state: "none", rows: [] })} runId={RUN} />);
    expect(screen.getByText("No receipt in the PR body yet.")).toBeTruthy();
    cleanup();
    render(<ReceiptSection pr={pr({ state: "unreadable", rows: [] })} runId={RUN} />);
    expect(screen.getByText("Receipt unreadable: PR1's body could not be read.")).toBeTruthy();
  });

  it("shows a row as text, never as HTML", () => {
    const hostile = "<b>bold</b> <img src=x onerror=alert(1)>";
    render(<ReceiptSection pr={pr({ state: "rows", rows: [hostile] })} runId={RUN} />);
    expect(document.querySelector("li")!.textContent).toBe(hostile);
    expect(document.querySelector("li b")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
  });

  it("opens the Trace from the trace-link row, and leaves another run's link as text", () => {
    const open = vi.fn();
    render(<ReceiptSection pr={pr(rows)} runId={RUN} onOpenTrace={open} />);
    fireEvent.click(screen.getByRole("button", { name: "Trace on the machine that ran it" }));
    expect(open).toHaveBeenCalledTimes(1);
    cleanup();
    render(<ReceiptSection pr={pr(rows)} runId="r-other" onOpenTrace={open} />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("renders nothing for a backend that sends no receipt", () => {
    render(<ReceiptSection pr={pr(undefined)} runId={RUN} />);
    expect(document.querySelector("details")).toBeNull();
  });
});
