// @vitest-environment jsdom
import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { TerminalSearchBar, type TerminalSearchLike } from "./TerminalSearchBar";

function fakeSearch(buffer: string) {
  let listener: ((e: { resultIndex: number; resultCount: number }) => void) | null = null;
  const count = (term: string, opts?: { caseSensitive?: boolean }) => {
    if (!term) return 0;
    const hay = opts?.caseSensitive ? buffer : buffer.toLowerCase();
    const needle = opts?.caseSensitive ? term : term.toLowerCase();
    return hay.split(needle).length - 1;
  };
  let index = -1;
  const find = (dir: 1 | -1) =>
    vi.fn((term: string, opts?: { caseSensitive?: boolean; incremental?: boolean }) => {
      const total = count(term, opts);
      if (total === 0) {
        index = -1;
      } else if (!(opts?.incremental && index >= 0)) {
        index = (index + dir + total) % total;
      }
      listener?.({ resultIndex: index, resultCount: total });
      return total > 0;
    });
  const search = {
    findNext: find(1),
    findPrevious: find(-1),
    clearDecorations: vi.fn(),
    onDidChangeResults: (fn: typeof listener) => {
      listener = fn;
      return { dispose: () => (listener = null) };
    },
  };
  return search as unknown as TerminalSearchLike & typeof search;
}

describe("TerminalSearchBar", () => {
  it("searches the buffer as you type, steps with Enter / Shift+Enter, and shows n of m", () => {
    const search = fakeSearch("npm test\nnpm run build\nNPM audit");
    const onClose = vi.fn();
    render(<TerminalSearchBar search={search} onClose={onClose} />);
    const input = screen.getByRole("searchbox", { name: "Search terminal" });
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: "npm" } });
    expect(search.findNext).toHaveBeenLastCalledWith(
      "npm",
      expect.objectContaining({ incremental: true, caseSensitive: false }),
    );
    expect(screen.getByText("1 of 3")).toBeTruthy();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(screen.getByText("2 of 3")).toBeTruthy();
    fireEvent.keyDown(input, { key: "Enter", shiftKey: true });
    expect(search.findPrevious).toHaveBeenCalled();
    expect(screen.getByText("1 of 3")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Match case" }));
    expect(search.findNext).toHaveBeenLastCalledWith(
      "npm",
      expect.objectContaining({ caseSensitive: true }),
    );
    expect(screen.getByText(/of 2/)).toBeTruthy();

    fireEvent.change(input, { target: { value: "yarn" } });
    expect(screen.getByText("No results")).toBeTruthy();

    fireEvent.keyDown(input, { key: "Escape" });
    expect(search.clearDecorations).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it("reports a bad regular expression instead of throwing", () => {
    const search = fakeSearch("abc");
    search.findNext.mockImplementation(() => {
      throw new SyntaxError("Invalid regular expression");
    });
    render(<TerminalSearchBar search={search} onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Regular expression" }));
    act(() => {
      fireEvent.change(screen.getByRole("searchbox", { name: "Search terminal" }), {
        target: { value: "(" },
      });
    });
    expect(screen.getByText("Invalid pattern")).toBeTruthy();
  });

  it("closes on Escape from the input or a button in the bar", () => {
    const onClose = vi.fn();
    render(<TerminalSearchBar search={fakeSearch("abc")} onClose={onClose} />);
    fireEvent.keyDown(screen.getByRole("searchbox", { name: "Search terminal" }), {
      key: "Escape",
    });
    expect(onClose).toHaveBeenCalledOnce();
    fireEvent.keyDown(screen.getByRole("button", { name: "Next match" }), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
  });
});
