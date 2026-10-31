// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { Meter } from "./Meter";

describe("Meter", () => {
  afterEach(cleanup);

  it("fills to the value and names it", () => {
    render(<Meter value={42} tone="warn" label="Claude 5h 42%" testId="m" />);
    const meter = screen.getByTestId("m");
    expect(meter.getAttribute("role")).toBe("meter");
    expect(meter.getAttribute("aria-valuenow")).toBe("42");
    expect(meter.getAttribute("aria-valuemin")).toBe("0");
    expect(meter.getAttribute("aria-valuemax")).toBe("100");
    expect(meter.getAttribute("aria-label")).toBe("Claude 5h 42%");
    expect(meter.className).toContain("tone-warn");
    expect((meter.querySelector(".meter-fill") as HTMLElement).style.width).toBe("42%");
  });

  it("clamps the fill to 0-100", () => {
    render(<Meter value={250} tone="danger" label="x" testId="m" />);
    expect(screen.getByTestId("m").getAttribute("aria-valuenow")).toBe("100");
  });

  it("draws unknown hatched with no value and no fill, never 0", () => {
    render(<Meter value={null} tone="danger" label="Claude 5h" testId="m" />);
    const meter = screen.getByTestId("m");
    expect(meter.className).toContain("meter-unknown");
    expect(meter.className).toContain("tone-muted");
    expect(meter.hasAttribute("aria-valuenow")).toBe(false);
    expect(meter.getAttribute("aria-label")).toContain("unknown");
    expect(meter.querySelector(".meter-fill")).toBeNull();
  });

  it("places the marker", () => {
    render(<Meter value={65} tone="warn" label="Context" marker={60} testId="m" />);
    expect((screen.getByTestId("m").querySelector(".meter-marker") as HTMLElement).style.left).toBe("60%");
    cleanup();
    render(<Meter value={65} tone="warn" label="Context" testId="m" />);
    expect(screen.getByTestId("m").querySelector(".meter-marker")).toBeNull();
  });
});
