// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Divider } from "./Divider";

afterEach(cleanup);

function view(over: Partial<React.ComponentProps<typeof Divider>> = {}) {
  const props = {
    orientation: "vertical" as const,
    value: 0.5,
    min: 0.2,
    max: 0.8,
    defaultValue: 0.5,
    label: "Resize",
    measure: (x: number) => x / 1000,
    onLive: vi.fn(),
    onCommit: vi.fn(),
    ...over,
  };
  render(<Divider {...props} />);
  return props;
}

describe("Divider", () => {
  it("has the separator ARIA values in percent", () => {
    view();
    const sep = screen.getByRole("separator", { name: "Resize" });
    expect(sep.getAttribute("aria-orientation")).toBe("vertical");
    expect(sep.getAttribute("aria-valuenow")).toBe("50");
    expect(sep.getAttribute("aria-valuemin")).toBe("20");
    expect(sep.getAttribute("aria-valuemax")).toBe("80");
  });
  it("drags with a live value and commits only on release", () => {
    const props = view();
    const sep = screen.getByRole("separator");
    fireEvent.pointerDown(sep, { pointerId: 1, clientX: 500 });
    fireEvent.pointerMove(sep, { clientX: 300 });
    expect(props.onLive).toHaveBeenLastCalledWith(0.3);
    fireEvent.pointerMove(sep, { clientX: 10 });
    expect(props.onLive).toHaveBeenLastCalledWith(0.2);
    expect(props.onCommit).not.toHaveBeenCalled();
    fireEvent.pointerUp(sep);
    expect(props.onLive).toHaveBeenLastCalledWith(null);
    expect(props.onCommit).toHaveBeenCalledExactlyOnceWith(0.2);
  });
  it("moves by arrow keys along its axis only, clamped", () => {
    const props = view();
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowRight" });
    expect(props.onCommit).toHaveBeenLastCalledWith(0.55);
    fireEvent.keyDown(sep, { key: "ArrowLeft" });
    expect(props.onCommit).toHaveBeenLastCalledWith(0.45);
    fireEvent.keyDown(sep, { key: "ArrowDown" });
    expect(props.onCommit).toHaveBeenCalledTimes(2);
  });
  it("uses up and down for a horizontal divider and clamps at the edges", () => {
    const props = view({ orientation: "horizontal", value: 0.78 });
    const sep = screen.getByRole("separator");
    fireEvent.keyDown(sep, { key: "ArrowDown" });
    expect(props.onCommit).toHaveBeenLastCalledWith(0.8);
    fireEvent.keyDown(sep, { key: "ArrowLeft" });
    expect(props.onCommit).toHaveBeenCalledTimes(1);
  });
  it("resets to the default on double-click", () => {
    const props = view({ value: 0.3, defaultValue: 0.58 });
    fireEvent.doubleClick(screen.getByRole("separator"));
    expect(props.onCommit).toHaveBeenCalledExactlyOnceWith(0.58);
  });
  it("ignores a pointer position that cannot be measured", () => {
    const props = view({ measure: () => null });
    const sep = screen.getByRole("separator");
    fireEvent.pointerDown(sep, { pointerId: 1 });
    fireEvent.pointerMove(sep, { clientX: 5 });
    expect(props.onLive).not.toHaveBeenCalled();
  });
});
