// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { readSource } from "../readSource";
import { ErrorBoundary } from "./ErrorBoundary";
import { TitleBar } from "./TitleBar";

afterEach(cleanup);

const drag = (el: Element) => el.hasAttribute("data-tauri-drag-region");

describe("TitleBar", () => {
  it("renders the row with no tabs, with a spacer and the search button", () => {
    const { container } = render(<TitleBar platform="linux" onOpenPalette={() => {}} />);
    expect(container.querySelector(".title-bar")).not.toBeNull();
    expect(container.querySelector(".title-bar-spacer")).not.toBeNull();
    expect(screen.getByRole("button", { name: "Search commands" })).toBeTruthy();
  });

  it("puts the drag attribute on the spacer and the account label only", () => {
    const { container } = render(
      <TitleBar platform="mac" accountName="  Work  " onOpenPalette={() => {}}>
        <div className="tab-bar" role="tablist">
          <div role="tab">One</div>
          <button type="button">New</button>
        </div>
      </TitleBar>,
    );
    const dragging = [...container.querySelectorAll("*")].filter(drag).map((el) => el.className);
    expect(dragging.sort()).toEqual(["title-bar-account", "title-bar-spacer"]);
    expect(screen.getByRole("tab").hasAttribute("data-tauri-drag-region")).toBe(false);
    for (const button of screen.getAllByRole("button")) expect(drag(button)).toBe(false);
    expect(screen.getByText("Work")).toBeTruthy();
  });

  it("shows nothing for an empty account name", () => {
    const { container } = render(<TitleBar platform="linux" accountName="   " onOpenPalette={() => {}} />);
    expect(container.querySelector(".title-bar-account")).toBeNull();
  });

  it("opens the palette from the search button", () => {
    const open = vi.fn();
    render(<TitleBar platform="mac" onOpenPalette={open} />);
    fireEvent.click(screen.getByRole("button", { name: "Search commands" }));
    expect(open).toHaveBeenCalledOnce();
    expect(screen.getByText("⌘K")).toBeTruthy();
  });

  it("shows the key table's label on windows, not a made-up one", () => {
    render(<TitleBar platform="windows" onOpenPalette={() => {}} />);
    expect(screen.getByText("Ctrl+Shift+K")).toBeTruthy();
  });

  it("reserves the traffic-light inset on mac only", () => {
    const mac = render(<TitleBar platform="mac" onOpenPalette={() => {}} />);
    expect(mac.container.querySelector(".title-bar-mac")).not.toBeNull();
    mac.unmount();
    const other = render(<TitleBar platform="windows" onOpenPalette={() => {}} />);
    expect(other.container.querySelector(".title-bar-mac")).toBeNull();
  });
});

describe("the window error screen", () => {
  it("has a drag strip so it still drags under the overlay title bar", () => {
    const Boom = () => {
      throw new Error("boom");
    };
    vi.spyOn(console, "error").mockImplementation(() => {});
    const { container } = render(
      <ErrorBoundary kind="window">
        <Boom />
      </ErrorBoundary>,
    );
    expect(container.querySelector(".error-boundary-drag[data-tauri-drag-region]")).not.toBeNull();
    vi.restoreAllMocks();
  });
});

describe("title row CSS", () => {
  const css = readSource("App.css").replace(/\/\*[\s\S]*?\*\//g, "");
  const body = (selector: string) =>
    new RegExp(`\\n${selector.replace(/[.[\]]/g, "\\$&")} \\{([^}]*)\\}`).exec(css)?.[1] ?? "";

  it("lets the tab list shrink and scroll while the spacer and the search button keep their room", () => {
    expect(body(".title-bar .tab-bar")).toMatch(/flex: 0 1 auto/);
    expect(body(".title-bar .tab-bar")).toMatch(/min-width: 0/);
    expect(body(".title-bar-spacer")).toMatch(/flex: 1 0 2rem/);
    expect(body(".title-bar-search")).toMatch(/flex: none/);
    expect(body(".title-bar-account")).toMatch(/flex: none/);
    expect(body(".tab-bar")).toMatch(/overflow-x: auto/);
  });

  it("insets the window error screen by the title row", () => {
    expect(body(".error-boundary-window")).toMatch(/padding-top: var\(--titlebar-h\)/);
  });
});

describe("capabilities", () => {
  it("grants starting a window drag", () => {
    expect(JSON.parse(readSource("../src-tauri/capabilities/default.json")).permissions).toContain("core:window:allow-start-dragging");
  });
});
