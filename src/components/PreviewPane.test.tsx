// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import malicious from "../../fixtures/preview/malicious.md?raw";

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
const mermaidInit = vi.hoisted(() => vi.fn());
vi.mock("mermaid", () => ({
  default: {
    initialize: mermaidInit,
    render: vi.fn(async () => ({
      svg: '<svg onload="window.pwned=5"><script>window.pwned=6</script><a href="javascript:alert(1)"><text onclick="x()">A</text></a><foreignObject><iframe src="https://evil.example"></iframe></foreignObject></svg>',
    })),
  },
}));

import { openPreview } from "../preview/bus";
import { previewFit } from "../previewLayout";
import { UI_SCALE_EVENT } from "../uiScale";
import { Markdown, PreviewHost, PreviewPane } from "./PreviewPane";

const RUN = "/app/runs/r-1";
const preview = (text: string, mtimeMs = 1, path = `${RUN}/plan.md`) => ({ path, text, truncated: false, mtimeMs });

beforeEach(() => {
  invoke.mockReset();
  openUrl.mockReset();
  openUrl.mockResolvedValue(undefined);
  (window as unknown as { pwned?: number }).pwned = undefined;
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("a malicious Markdown file", () => {
  it("renders with no script, iframe, event-handler attribute or javascript: link in the DOM", async () => {
    render(<Markdown text={malicious} from={`${RUN}/plan.md`} onNavigate={vi.fn()} />);
    await screen.findByTestId("mermaid");
    await waitFor(() => expect(document.querySelector("[data-testid=mermaid] svg")).not.toBeNull());
    const html = document.body.innerHTML;
    expect(document.querySelectorAll("script, iframe, object, embed, img, foreignObject")).toHaveLength(0);
    for (const el of document.querySelectorAll("*")) {
      for (const attr of el.getAttributeNames()) expect(attr.startsWith("on"), `${el.tagName} ${attr}`).toBe(false);
    }
    expect(html.toLowerCase()).not.toContain("javascript:");
    expect(html).not.toContain("data:text/html");
    expect(html).not.toContain("file:///");
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    // The raw HTML shows as text, so what the file says is still readable.
    expect(screen.getByTestId("preview-markdown").textContent).toContain("<iframe");
  });
});

describe("links never navigate the app", () => {
  const renderLinks = (onNavigate = vi.fn()) => {
    render(<Markdown text={malicious} from={`${RUN}/plans/plan.md`} onNavigate={onNavigate} />);
    return onNavigate;
  };

  it("opens an https link in the browser and leaves window.location alone", () => {
    const before = window.location.href;
    renderLinks();
    const click = fireEvent.click(screen.getByText("web link"));
    expect(click).toBe(false); // preventDefault was called
    expect(openUrl).toHaveBeenCalledWith("https://example.com/page");
    expect(window.location.href).toBe(before);
  });

  it("resolves a relative link against the file's folder and opens it in the preview", () => {
    const onNavigate = renderLinks();
    fireEvent.click(screen.getByText("relative link"));
    expect(onNavigate).toHaveBeenCalledWith(`${RUN}/plans/other.md`);
    expect(openUrl).not.toHaveBeenCalled();
  });

  it("does nothing for javascript:, data: and file: links: they are not links at all", () => {
    const onNavigate = renderLinks();
    for (const name of ["js link", "data link", "file link"]) {
      const node = screen.getByText(name);
      expect(node.tagName).toBe("SPAN");
      fireEvent.click(node);
    }
    expect(openUrl).not.toHaveBeenCalled();
    expect(onNavigate).not.toHaveBeenCalled();
  });

  it("an anchor link scrolls inside the pane", () => {
    renderLinks();
    const heading = document.getElementById("details")!;
    heading.scrollIntoView = vi.fn();
    fireEvent.click(screen.getByText("anchor link"));
    expect(heading.scrollIntoView).toHaveBeenCalled();
  });
});

describe("PreviewPane", () => {
  it("shows a refused file with the reason, and a relative link goes through preview_file with the resolved path", async () => {
    invoke.mockImplementation(async (cmd: string, args: { path: string }) => {
      if (cmd === "preview_stat") return { exists: true, mtimeMs: 1, size: 1 };
      if (args.path === `${RUN}/plan.md`) return preview("[next](other.md) and [out](../../../etc/hosts)");
      throw "outside the folders a preview may read";
    });
    render(<PreviewPane target={{ path: `${RUN}/plan.md`, title: "Plan" }} onClose={vi.fn()} />);
    fireEvent.click(await screen.findByText("next"));
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("preview_file", { path: `${RUN}/other.md` }));
    expect((await screen.findByTestId("preview-refused")).textContent).toContain("outside the folders");
  });

  it("re-renders within 2 s of a change and keeps the scroll", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "requestAnimationFrame", "cancelAnimationFrame"] });
    let mtime = 1;
    let text = "# One";
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "preview_stat" ? { exists: true, mtimeMs: mtime, size: 1 } : preview(text, mtime),
    );
    render(<PreviewPane target={{ path: `${RUN}/plan.md`, title: "Plan" }} onClose={vi.fn()} />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByText("One")).toBeTruthy();
    const body = document.querySelector(".preview-body") as HTMLElement;
    body.scrollTop = 120;
    mtime = 2;
    text = "# Two";
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1100);
    });
    expect(screen.getByText("Two")).toBeTruthy();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(50);
    });
    expect(body.scrollTop).toBe(120);
  });

  it("says when the file was removed, and when it is truncated", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "requestAnimationFrame", "cancelAnimationFrame"] });
    let exists = true;
    invoke.mockImplementation(async (cmd: string) =>
      cmd === "preview_stat" ? { exists, mtimeMs: 1, size: 1 } : { ...preview("# Big"), truncated: true },
    );
    render(<PreviewPane target={{ path: `${RUN}/plan.md`, title: "Plan" }} onClose={vi.fn()} />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByTestId("preview-truncated")).toBeTruthy();
    exists = false;
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1100);
    });
    expect(screen.getByTestId("preview-removed")).toBeTruthy();
  });

  it("renders a large file within a time budget", async () => {
    const text = Array.from({ length: 4000 }, (_, i) => `Line ${i} with **bold** and a [link](https://example.com/${i})`).join("\n\n");
    const started = performance.now();
    render(<Markdown text={text} from={`${RUN}/big.md`} onNavigate={vi.fn()} />);
    expect(performance.now() - started).toBeLessThan(5000);
    expect(screen.getAllByText(/Line \d+ with/).length).toBeGreaterThan(100);
  });
});

describe("document layout", () => {
  it("wraps a table in a sideways-scrolling block so cells keep their width", () => {
    render(<Markdown text={"| Area | Rule |\n| --- | --- |\n| Import | One renderer |"} from={`${RUN}/a.md`} onNavigate={vi.fn()} />);
    const table = document.querySelector(".preview-table > table");
    expect(table).not.toBeNull();
    expect(table?.querySelectorAll("th")).toHaveLength(2);
  });

  it("draws Mermaid labels as SVG text, since the sanitiser removes foreignObject labels", async () => {
    mermaidInit.mockClear();
    render(<Markdown text={"```mermaid\nflowchart TD\n  A[Parent] --> B[Child]\n```"} from={`${RUN}/a.md`} onNavigate={vi.fn()} />);
    await waitFor(() => expect(mermaidInit).toHaveBeenCalled());
    expect(mermaidInit.mock.calls[0][0]).toMatchObject({ securityLevel: "strict", htmlLabels: false, flowchart: { htmlLabels: false } });
  });

  it("has no Wider button and no divider as a bare overlay", () => {
    invoke.mockImplementation(async (cmd: string) => (cmd === "preview_stat" ? { exists: true, mtimeMs: 1, size: 1 } : preview("# Doc")));
    render(<PreviewPane target={{ path: `${RUN}/plan.md`, title: "Plan" }} onClose={vi.fn()} />);
    expect(screen.queryByText("Wider")).toBeNull();
    expect(screen.queryByTestId("preview-wide")).toBeNull();
    expect(screen.queryByRole("separator")).toBeNull();
    expect(screen.getByTestId("preview-pane").classList.contains("preview-overlay")).toBe(true);
  });
});

describe("a path clicked in a terminal that the preview refuses", () => {
  const OUTSIDE = "outside the folders a preview may read";
  const openCalls = () => invoke.mock.calls.filter(([cmd]) => cmd === "open_file_link");
  const clicked = { path: "/etc/hosts", title: "hosts", tabId: "t1", line: 3, col: 4 };
  const respond = (refusal: string) =>
    invoke.mockImplementation(async (cmd: string, args: { path?: string }) => {
      if (cmd === "preview_stat") return { exists: true, mtimeMs: 1, size: 1 };
      if (cmd === "open_file_link") return null;
      if (args.path === clicked.path) throw refusal;
      return preview("# Other", 1, args.path);
    });

  it("shows the refusal and 'Opened in your editor.' and opens the editor once, even after poll ticks", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "requestAnimationFrame", "cancelAnimationFrame"] });
    respond(OUTSIDE);
    render(<PreviewPane target={clicked} onClose={vi.fn()} />);
    await act(async () => {
      await Promise.resolve();
    });
    const refused = screen.getByTestId("preview-refused");
    expect(refused.textContent).toContain(OUTSIDE);
    expect(refused.textContent).toContain("Opened in your editor.");
    expect(openCalls()).toEqual([["open_file_link", { tabId: "t1", path: "/etc/hosts", line: 3, col: 4, editor: "finder" }]]);
    for (let tick = 0; tick < 4; tick += 1) {
      await act(async () => {
        vi.advanceTimersByTime(1100);
      });
    }
    expect(invoke.mock.calls.filter(([cmd]) => cmd === "preview_file").length).toBeGreaterThan(2);
    expect(openCalls()).toHaveLength(1);
    expect(screen.getByTestId("preview-refused").textContent).toContain("Opened in your editor.");
  });

  it("falls back for a dotfile too", async () => {
    respond("a dotfile is not previewed");
    render(<PreviewPane target={clicked} onClose={vi.fn()} />);
    await waitFor(() => expect(openCalls()).toHaveLength(1));
  });

  it("shows the refusal only for a missing file and a binary file", async () => {
    for (const reason of ["file not found", "a binary file is not previewed"]) {
      invoke.mockReset();
      respond(reason);
      const { unmount } = render(<PreviewPane target={clicked} onClose={vi.fn()} />);
      const refused = await screen.findByTestId("preview-refused");
      expect(refused.textContent).toContain(reason);
      expect(refused.textContent).not.toContain("Opened in your editor.");
      expect(openCalls()).toHaveLength(0);
      unmount();
    }
  });

  it("does not open the editor for a link refused inside the pane, even though the target has a tab", async () => {
    invoke.mockImplementation(async (cmd: string, args: { path?: string }) => {
      if (cmd === "preview_stat") return { exists: true, mtimeMs: 1, size: 1 };
      if (cmd === "open_file_link") return null;
      if (args.path === `${RUN}/plan.md`) return preview("[next](other.md)");
      throw OUTSIDE;
    });
    render(<PreviewPane target={{ path: `${RUN}/plan.md`, title: "Plan", tabId: "t1" }} onClose={vi.fn()} />);
    fireEvent.click(await screen.findByText("next"));
    const refused = await screen.findByTestId("preview-refused");
    expect(refused.textContent).toContain(OUTSIDE);
    expect(refused.textContent).not.toContain("Opened in your editor.");
    expect(openCalls()).toHaveLength(0);
  });

  it("loads again when the same path is clicked twice", async () => {
    respond(OUTSIDE);
    const { rerender } = render(<PreviewPane target={clicked} onClose={vi.fn()} />);
    await waitFor(() => expect(openCalls()).toHaveLength(1));
    const loads = () => invoke.mock.calls.filter(([cmd]) => cmd === "preview_file").length;
    const before = loads();
    rerender(<PreviewPane target={{ ...clicked }} onClose={vi.fn()} />);
    await waitFor(() => expect(loads()).toBe(before + 1));
    await waitFor(() => expect(openCalls()).toHaveLength(2));
  });
});

describe("PreviewHost", () => {
  const TARGET = { path: `${RUN}/plan.md`, title: "Plan" };
  const BASE = { cellWidth: 0.6 * 14, termPad: 6, minPreviewPx: 320 };
  let row: HTMLDivElement;
  let rowWidth = 1400;
  const observers: Array<() => void> = [];
  const originalObserver = globalThis.ResizeObserver;
  const originalInner = window.innerWidth;

  beforeEach(() => {
    invoke.mockImplementation(async (cmd: string) => (cmd === "preview_stat" ? { exists: true, mtimeMs: 1, size: 1 } : preview("# Doc")));
    window.localStorage.clear();
    document.documentElement.style.fontSize = "16px";
    Object.defineProperty(window, "innerWidth", { value: 1400, configurable: true });
    rowWidth = 1400;
    row = document.createElement("div");
    document.body.appendChild(row);
    vi.spyOn(row, "getBoundingClientRect").mockImplementation(() => ({ left: 0, top: 0, width: rowWidth, height: 600 }) as DOMRect);
    observers.length = 0;
    globalThis.ResizeObserver = class {
      constructor(cb: () => void) {
        observers.push(cb);
      }
      observe() {}
      disconnect() {}
    } as never;
  });
  afterEach(() => {
    row.remove();
    document.documentElement.style.fontSize = "";
    Object.defineProperty(window, "innerWidth", { value: originalInner, configurable: true });
    globalThis.ResizeObserver = originalObserver;
    window.localStorage.clear();
  });

  const gutter = (px = 16) => 8 + 0.3125 * px;
  const widthOf = (over: Record<string, number> = {}) => {
    const fit = previewFit({
      ratio: 0.42,
      windowWidth: 1400,
      rowWidth,
      runPanelWidth: 0,
      gutter: gutter(),
      ...BASE,
      ...over,
    });
    if (fit.mode !== "side") throw new Error("expected side");
    return fit.width;
  };
  const mount = (props: Partial<React.ComponentProps<typeof PreviewHost>> = {}) => {
    const onLayoutChange = vi.fn();
    const ui = (extra: Partial<React.ComponentProps<typeof PreviewHost>> = {}) => (
      <PreviewHost windowId="main" terminalId="t1" fontSize={14} rowRef={{ current: row }} onLayoutChange={onLayoutChange} {...props} {...extra} />
    );
    const result = render(ui());
    act(() => openPreview(TARGET));
    return { ...result, ui, onLayoutChange };
  };
  const pane = () => screen.getByTestId("preview-pane");

  it("opens beside the terminal with a divider, the fitted width as its flex basis, and no Wider button", () => {
    mount();
    expect(pane().style.flex).toBe(`0 0 ${widthOf()}px`);
    expect(pane().classList.contains("preview-overlay")).toBe(false);
    expect(screen.getByRole("separator", { name: "Resize preview" })).toBeTruthy();
    expect(screen.queryByText("Wider")).toBeNull();
  });

  it("is an overlay with no divider under 1000px of window", () => {
    Object.defineProperty(window, "innerWidth", { value: 999, configurable: true });
    mount();
    expect(pane().classList.contains("preview-overlay")).toBe(true);
    expect(pane().style.flex).toBe("");
    expect(screen.queryByRole("separator")).toBeNull();
  });

  it("writes the ratio store only on an arrow key, and the fit follows it", () => {
    mount();
    expect(window.localStorage.getItem("dcv2.preview.main")).toBeNull();
    fireEvent.keyDown(screen.getByRole("separator"), { key: "ArrowLeft" });
    const saved = JSON.parse(window.localStorage.getItem("dcv2.preview.main")!) as { ratio: number };
    expect(saved.ratio).toBeCloseTo(0.42 + 0.05, 2);
    expect(pane().style.flex).toBe(`0 0 ${widthOf({ ratio: saved.ratio })}px`);
  });

  it("re-fits on a window resize, a row resize, a UI scale change and a font size change, without writing the ratio", () => {
    window.localStorage.setItem("dcv2.preview.main", '{"ratio":0.85}');
    rowWidth = 1000;
    Object.defineProperty(window, "innerWidth", { value: 1000, configurable: true });
    const { ui, rerender, onLayoutChange } = mount();
    const first = pane().style.flex;
    expect(first).toBe(`0 0 ${widthOf({ ratio: 0.85, windowWidth: 1000 })}px`);

    rowWidth = 1200;
    act(() => observers.forEach((cb) => cb()));
    expect(pane().style.flex).toBe(`0 0 ${widthOf({ ratio: 0.85, windowWidth: 1000 })}px`);
    expect(pane().style.flex).not.toBe(first);

    Object.defineProperty(window, "innerWidth", { value: 900, configurable: true });
    act(() => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(pane().classList.contains("preview-overlay")).toBe(true);
    Object.defineProperty(window, "innerWidth", { value: 1400, configurable: true });
    act(() => {
      window.dispatchEvent(new Event("resize"));
    });
    const before = pane().style.flex;

    document.documentElement.style.fontSize = "20px";
    act(() => {
      window.dispatchEvent(new CustomEvent(UI_SCALE_EVENT, { detail: 125 }));
    });
    expect(pane().style.flex).not.toBe(before);
    expect(pane().style.flex).toBe(`0 0 ${widthOf({ ratio: 0.85, rowWidth: 1200, gutter: gutter(20), minPreviewPx: 400 })}px`);

    document.documentElement.style.fontSize = "16px";
    act(() => {
      window.dispatchEvent(new CustomEvent(UI_SCALE_EVENT, { detail: 100 }));
    });
    const small = pane().style.flex;
    rerender(ui({ fontSize: 18 }));
    expect(pane().style.flex).not.toBe(small);
    expect(pane().style.flex).toBe(`0 0 ${widthOf({ ratio: 0.85, rowWidth: 1200, cellWidth: 0.6 * 18 })}px`);
    expect(onLayoutChange).toBeDefined();
    expect(window.localStorage.getItem("dcv2.preview.main")).toBe('{"ratio":0.85}');
  });

  it("asks the terminals to refit when the mode or width changes", async () => {
    vi.useFakeTimers({ toFake: ["requestAnimationFrame", "cancelAnimationFrame"] });
    try {
      const { onLayoutChange } = mount();
      await act(async () => {
        vi.advanceTimersByTime(50);
      });
      onLayoutChange.mockClear();
      rowWidth = 1300;
      act(() => observers.forEach((cb) => cb()));
      await act(async () => {
        vi.advanceTimersByTime(50);
      });
      expect(onLayoutChange).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
