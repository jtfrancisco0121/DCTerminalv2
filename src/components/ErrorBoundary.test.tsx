// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

function Boom({ fail }: { fail: boolean }) {
  if (fail) throw new Error("render exploded");
  return <p>fine</p>;
}

describe("ErrorBoundary", () => {
  beforeEach(() => {
    // React logs a caught render error; keep the test output clean.
    vi.spyOn(console, "error").mockImplementation(() => {});
  });
  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("shows the title, the message and a focused Close for a throwing page", () => {
    const onClose = vi.fn();
    render(
      <ErrorBoundary kind="page" onClose={onClose}>
        <Boom fail />
      </ErrorBoundary>,
    );
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("This page failed");
    expect(alert.textContent).toContain("render exploded");
    expect(alert.textContent).toContain("The rest of the window is still running.");
    const close = screen.getByRole("button", { name: "Close" });
    expect(document.activeElement).toBe(close);
    fireEvent.click(close);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Copy error" })).toBeTruthy();
  });

  it("names a dialog and the window in their own words", () => {
    render(
      <ErrorBoundary kind="dialog" onClose={() => {}}>
        <Boom fail />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert").textContent).toContain("This dialog failed");
    cleanup();
    render(
      <ErrorBoundary kind="window">
        <Boom fail />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert").textContent).toContain("Something failed in this window");
  });

  it("renders the child fresh after a remount under a new key", () => {
    const { rerender } = render(
      <ErrorBoundary key="a" kind="page" onClose={() => {}}>
        <Boom fail />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert")).toBeTruthy();
    rerender(
      <ErrorBoundary key="b" kind="page" onClose={() => {}}>
        <Boom fail={false} />
      </ErrorBoundary>,
    );
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByText("fine")).toBeTruthy();
  });

  it("keeps the panel when the child fails again", () => {
    const { rerender } = render(
      <ErrorBoundary kind="page" onClose={() => {}}>
        <Boom fail />
      </ErrorBoundary>,
    );
    rerender(
      <ErrorBoundary kind="page" onClose={() => {}}>
        <Boom fail />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert").textContent).toContain("render exploded");
  });
});

const sources = import.meta.glob("/src/{App,main}.tsx", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

describe("App wiring", () => {
  const app = sources["/src/App.tsx"]!;
  const main = sources["/src/main.tsx"]!;

  it("never wraps the terminal tree in a boundary", () => {
    // Every terminal render site sits outside every <ErrorBoundary> ... </ErrorBoundary> span.
    const spans: Array<[number, number]> = [];
    const stack: number[] = [];
    for (const m of app.matchAll(/<ErrorBoundary\b|<\/ErrorBoundary>/g)) {
      if (m[0] === "</ErrorBoundary>") spans.push([stack.pop()!, m.index!]);
      else stack.push(m.index!);
    }
    expect(spans.length).toBeGreaterThan(10);
    for (const m of app.matchAll(/terminalsOf\(|className="run-layout"/g)) {
      expect(spans.some(([a, b]) => m.index! > a && m.index! < b)).toBe(false);
    }
  });

  it("main.tsx keeps one window-level boundary around App", () => {
    expect(main).toContain('<ErrorBoundary kind="window">');
  });
});
