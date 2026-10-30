// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import malicious from "../../fixtures/preview/malicious.md?raw";

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn(async () => ({
      svg: '<svg onload="window.pwned=5"><script>window.pwned=6</script><a href="javascript:alert(1)"><text onclick="x()">A</text></a><foreignObject><iframe src="https://evil.example"></iframe></foreignObject></svg>',
    })),
  },
}));

import { Markdown, PreviewPane } from "./PreviewPane";

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
