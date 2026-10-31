// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { TrendPoint } from "../run/analytics";
import { BarChart, LineChart } from "./Chart";

const point = (id: string, day: number, value: number): TrendPoint => ({ runId: id, at: `2026-10-${day}T10:00:00Z`, label: `Run ${id}`, value });
const usd = (v: number) => `$${v.toFixed(2)}`;

describe("charts", () => {
  afterEach(cleanup);

  it("a bar chart is an image named with its range, with one focusable titled bar per run", () => {
    const { container } = render(<BarChart title="Cost" points={[point("a", 20, 1), point("b", 22, 3)]} format={usd} />);
    const chart = screen.getByRole("img");
    expect(chart.getAttribute("aria-label")).toBe("Cost: 2 runs from 2026-10-20 to 2026-10-22, values from $1.00 to $3.00");
    const bars = container.querySelectorAll("rect.chart-bar");
    expect(bars).toHaveLength(2);
    expect(bars[0].getAttribute("tabindex")).toBe("0");
    expect(bars[1].querySelector("title")?.textContent).toBe("Run b · 2026-10-22 · $3.00");
    // The tallest bar fills the plot; the other is a third of it.
    expect(Number(bars[0].getAttribute("height")) / Number(bars[1].getAttribute("height"))).toBeCloseTo(1 / 3);
  });

  it("a line chart draws a line and a titled dot per run on a 0 to 100% scale", () => {
    const { container } = render(<LineChart title="Wait" points={[point("a", 20, 0.25), point("b", 21, 1)]} format={(v) => `${Math.round(v * 100)}%`} />);
    expect(screen.getByRole("img").getAttribute("aria-label")).toContain("values from 25% to 100%");
    expect(container.querySelector("polyline.chart-line")).toBeTruthy();
    expect(container.querySelectorAll("circle.chart-dot")).toHaveLength(2);
    expect(container.querySelector(".chart-axis")?.textContent).toContain("100%");
  });

  it("fewer than two points is a sentence, not a chart", () => {
    render(<BarChart title="Cost" points={[point("a", 20, 1)]} format={usd} />);
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByText(/at least two finished runs/)).toBeTruthy();
    cleanup();
    render(<LineChart title="Wait" points={[]} format={usd} />);
    expect(screen.queryByRole("img")).toBeNull();
  });
});
