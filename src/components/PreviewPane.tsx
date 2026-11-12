import { openUrl } from "@tauri-apps/plugin-opener";
import { Children, isValidElement, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { previewFile, previewStat, type Preview } from "../preview/api";
import { PREVIEW_EVENT, resolveRelative, slug, type PreviewTarget } from "../preview/bus";
import { MermaidBlock } from "./MermaidBlock";
import { Divider } from "./Divider";
import {
  DEFAULT_PREVIEW_RATIO,
  loadPreviewRatio,
  MAX_PREVIEW_RATIO,
  MIN_PREVIEW_RATIO,
  previewAvailable,
  previewFit,
  savePreviewRatio,
  type PreviewFit,
} from "../previewLayout";
import { openInEditor, terminalCellWidth } from "../terminal/park";
import { PREVIEW_EDITOR_FALLBACK_REASONS, PREVIEW_OPENED_IN_EDITOR } from "../run/words";
import { UI_SCALE_EVENT } from "../uiScale";
import { baseName } from "../paths";
import { errorText } from "./InlineNotice";
import { reportError } from "../uiErrors";

const POLL_MS = 1000;

type View =
  | { kind: "loading" }
  | { kind: "refused"; reason: string; openedInEditor?: boolean }
  | { kind: "removed" }
  | { kind: "ok"; preview: Preview };

function textOf(node: ReactNode): string {
  return Children.toArray(node)
    .map((child) => (typeof child === "string" || typeof child === "number" ? String(child) : isValidElement<{ children?: ReactNode }>(child) ? textOf(child.props.children) : ""))
    .join("");
}

/**
 * The text of a previewed file, rendered. No raw HTML (react-markdown shows it as text), every link is
 * handled here and never navigates the webview, and a Mermaid block is the only HTML inserted.
 */
export function Markdown({ text, from, onNavigate }: { text: string; from: string; onNavigate: (path: string) => void }) {
  const root = useRef<HTMLDivElement>(null);
  const components = useMemo(
    () => ({
      a({ href, children }: { href?: string; children?: ReactNode }) {
        const target = href ?? "";
        const onClick = (event: React.MouseEvent) => {
          event.preventDefault();
          event.stopPropagation();
          if (target.startsWith("#")) {
            const want = decodeURIComponent(target.slice(1)).toLowerCase();
            const hit = [...(root.current?.querySelectorAll("h1,h2,h3,h4,h5,h6") ?? [])].find((h) => h.id === want);
            hit?.scrollIntoView?.({ block: "start" });
          } else if (/^https?:\/\//i.test(target)) {
            void openUrl(target).catch((err: unknown) => reportError("Could not open link", err, { notify: true }));
          } else {
            const resolved = resolveRelative(from, target);
            if (resolved) onNavigate(resolved);
          }
        };
        const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(target)?.[1]?.toLowerCase();
        // Any other scheme (javascript:, data:, file:, ...) is plain text, not a link; react-markdown already blanks the
        // href of the unsafe ones, so an empty href is the same case.
        if (!target || (scheme && scheme !== "http" && scheme !== "https")) return <span className="preview-dead-link">{children}</span>;
        return (
          <a
            href={target.startsWith("#") || /^https?:/i.test(target) ? target : undefined}
            role="link"
            tabIndex={0}
            onClick={onClick}
            onKeyDown={(event) => {
              if (event.key === "Enter") onClick(event as unknown as React.MouseEvent);
            }}
            rel="noreferrer"
          >
            {children}
          </a>
        );
      },
      // No remote images: they would phone home when a file is merely read.
      img({ alt }: { alt?: string }) {
        return <span className="preview-dead-link">[image: {alt || "no description"}]</span>;
      },
      code({ className, children }: { className?: string; children?: ReactNode }) {
        if (/language-mermaid/.test(className ?? "")) return <MermaidBlock source={textOf(children).replace(/\n$/, "")} />;
        return <code className={className}>{children}</code>;
      },
      pre({ children }: { children?: ReactNode }) {
        const only = Children.toArray(children)[0];
        return isValidElement(only) && only.type === MermaidBlock ? <>{only}</> : <pre>{children}</pre>;
      },
      // Wide tables scroll sideways instead of squeezing every cell to one word per line.
      table({ children }: { children?: ReactNode }) {
        return (
          <div className="preview-table">
            <table>{children}</table>
          </div>
        );
      },
      ...Object.fromEntries(
        (["h1", "h2", "h3", "h4", "h5", "h6"] as const).map((tag) => [
          tag,
          ({ children }: { children?: ReactNode }) => {
            const Tag = tag;
            return <Tag id={slug(textOf(children))}>{children}</Tag>;
          },
        ]),
      ),
    }),
    [from, onNavigate],
  );
  return (
    <div ref={root} className="preview-markdown" data-testid="preview-markdown">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {text}
      </ReactMarkdown>
    </div>
  );
}

/** What the pane's divider needs from its host: the divider's share from the row's left edge, and where it goes. */
export type PreviewDivider = {
  value: number;
  min: number;
  max: number;
  defaultValue: number;
  measure: (clientX: number, clientY: number) => number | null;
  onLive: (value: number | null) => void;
  onCommit: (value: number) => void;
};

/**
 * The preview beside the terminal (or over it): one file, re-read within about a second of a change, scroll kept.
 * Its width is the flex basis the host fitted; it has no width of its own.
 */
export function PreviewPane({
  target,
  onClose,
  fit = { mode: "overlay" },
  divider,
}: {
  target: PreviewTarget;
  onClose: () => void;
  fit?: PreviewFit;
  divider?: PreviewDivider;
}) {
  // One request per file shown: a new target, or a link followed in the pane. Only a target clicked in a terminal
  // (it has a `tabId`) carries `click`, and only the load of that request may fall back to the editor.
  const [request, setRequest] = useState<{ path: string; click: PreviewTarget | null }>(() => ({
    path: target.path,
    click: target.tabId ? target : null,
  }));
  const path = request.path;
  const [title, setTitle] = useState(target.title);
  const [view, setView] = useState<View>({ kind: "loading" });
  const scroller = useRef<HTMLDivElement>(null);
  const seen = useRef(0);
  // The path the editor was opened for by this request's click, so a poll's refusal keeps saying so.
  const openedFor = useRef<string | null>(null);
  const lastTarget = useRef(target);
  // A link followed inside the pane: same target, no click.
  const navigate = useCallback((next: string) => setRequest({ path: next, click: null }), []);

  useEffect(() => {
    if (lastTarget.current === target) return;
    lastTarget.current = target;
    setTitle(target.title);
    // A fresh request even for the path already shown: re-clicking it loads again.
    setRequest({ path: target.path, click: target.tabId ? target : null });
  }, [target]);

  const load = useCallback((at: string, keepScroll: boolean, click: PreviewTarget | null = null) => {
    const top = scroller.current?.scrollTop ?? 0;
    previewFile(at).then(
      (preview) => {
        seen.current = preview.mtimeMs;
        setView({ kind: "ok", preview });
        if (keepScroll) window.requestAnimationFrame(() => scroller.current && (scroller.current.scrollTop = top));
      },
      (err: unknown) => {
        const reason = errorText(err);
        if (click?.tabId && at === click.path && PREVIEW_EDITOR_FALLBACK_REASONS.some((known) => reason.includes(known))) {
          openedFor.current = at;
          openInEditor(click.tabId, click.path, click.line, click.col);
        }
        setView({ kind: "refused", reason, openedInEditor: openedFor.current === at });
      },
    );
  }, []);

  useEffect(() => {
    setView({ kind: "loading" });
    seen.current = 0;
    openedFor.current = null;
    load(request.path, false, request.click);
  }, [request, load]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === "hidden") return;
      previewStat(path).then(
        (stat) => {
          if (!stat.exists) setView((v) => (v.kind === "removed" ? v : { kind: "removed" }));
          else if (stat.mtimeMs !== seen.current) load(path, true);
        },
        () => undefined,
      );
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [path, load]);

  const name = baseName(path) || path;
  return (
    <>
    {fit.mode === "side" && divider && (
      <Divider className="preview-divider" orientation="vertical" label="Resize preview" {...divider} />
    )}
    <aside
      className={fit.mode === "overlay" ? "preview-pane preview-overlay" : "preview-pane"}
      style={fit.mode === "side" ? { flex: `0 0 ${fit.width}px` } : undefined}
      aria-label={`Preview: ${title}`}
      data-testid="preview-pane"
    >
      <header className="preview-head">
        <strong title={path}>{title === name ? name : `${title} · ${name}`}</strong>
        <span className="preview-actions">
          <button type="button" className="link-button" onClick={onClose} aria-label="Close preview">
            Close
          </button>
        </span>
      </header>
      <div ref={scroller} className="preview-body">
        {view.kind === "loading" && <p className="hint">Loading…</p>}
        {view.kind === "refused" && (
          <p className="hint warn" role="alert" data-testid="preview-refused">
            This file cannot be previewed: {view.reason}.{view.openedInEditor && ` ${PREVIEW_OPENED_IN_EDITOR}`}
          </p>
        )}
        {view.kind === "removed" && (
          <p className="hint warn" role="status" data-testid="preview-removed">
            The file was removed.
          </p>
        )}
        {view.kind === "ok" && (
          <>
            {view.preview.truncated && (
              <p className="hint warn" data-testid="preview-truncated">
                The file is larger than 2 MB: only the start is shown.
              </p>
            )}
            <Markdown text={view.preview.text} from={view.preview.path} onNavigate={navigate} />
          </>
        )}
      </div>
    </aside>
    </>
  );
}

const rootPx = () => Number.parseFloat(window.getComputedStyle(document.documentElement).fontSize) || 16;
const cssPx = (row: Element, name: string, fallback: number) => {
  const value = Number.parseFloat(window.getComputedStyle(row).getPropertyValue(name));
  return Number.isFinite(value) ? value : fallback;
};
/** The divider's width: 0.3125rem, as `.preview-divider` in App.css. */
const DIVIDER_REM = 0.3125;
/** The narrowest the preview may be: 20 rem, read at fit time so it follows the UI scale. */
const MIN_PREVIEW_REM = 20;

/**
 * The one preview of the window, in the workspace row between the terminals and the run panel: it shows the file
 * `openPreview` asked for, in whichever tab is active (it keeps its target across tab switches), at the width
 * `previewFit` gives. The fit is recomputed on a window resize, a row resize (the run panel dragged or collapsed), a
 * UI scale change and a terminal font size change; it never touches the saved ratio.
 */
export function PreviewHost({
  windowId,
  terminalId,
  fontSize,
  rowRef,
  onLayoutChange,
}: {
  windowId: string;
  /** The active tab's terminal, for the cell width. */
  terminalId: string | null;
  fontSize: number;
  /** The workspace row that holds the terminals, this pane and the run panel. */
  rowRef: React.RefObject<HTMLElement | null>;
  onLayoutChange?: () => void;
}) {
  const [target, setTarget] = useState<PreviewTarget | null>(null);
  const [ratio, setRatio] = useState(() => loadPreviewRatio(windowId));
  const [live, setLive] = useState<number | null>(null);
  const [fit, setFit] = useState<PreviewFit>({ mode: "overlay" });
  const [available, setAvailable] = useState(0);
  useEffect(() => {
    const on = (event: Event) => setTarget((event as CustomEvent<PreviewTarget>).detail);
    window.addEventListener(PREVIEW_EVENT, on);
    return () => window.removeEventListener(PREVIEW_EVENT, on);
  }, []);
  const refit = useRef(onLayoutChange);
  refit.current = onLayoutChange;

  const inputs = () => {
    const row = rowRef.current;
    if (!row) return null;
    const rect = row.getBoundingClientRect();
    const panel = row.querySelector(".run-panel-frame, .run-rail");
    const gap = cssPx(row, "--gutter", 8);
    const px = rootPx();
    // Between the terminals and the preview lie one gap and the divider (it overlaps half a gap each side): that stretch is the gutter.
    return {
      windowWidth: window.innerWidth,
      rowWidth: rect.width,
      runPanelWidth: panel ? panel.getBoundingClientRect().width : 0,
      gutter: gap + DIVIDER_REM * px,
      cellWidth: (terminalId ? terminalCellWidth(terminalId) : null) ?? 0.6 * fontSize,
      termPad: cssPx(row, "--term-pad", 6),
      minPreviewPx: MIN_PREVIEW_REM * px,
    };
  };
  const recompute = () => {
    const measured = inputs();
    if (!measured) return;
    const next = previewFit({ ratio: live ?? ratio, ...measured });
    setAvailable(previewAvailable(measured));
    setFit((prev) => (prev.mode === next.mode && (prev.mode !== "side" || next.mode !== "side" || prev.width === next.width) ? prev : next));
  };
  const recomputeRef = useRef(recompute);
  recomputeRef.current = recompute;

  // The fit follows the window, the row, the UI scale, the font size, the saved ratio and the live drag.
  useEffect(() => {
    if (!target) return;
    const again = () => recomputeRef.current();
    window.addEventListener("resize", again);
    window.addEventListener(UI_SCALE_EVENT, again);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(again);
    if (rowRef.current) observer?.observe(rowRef.current);
    return () => {
      window.removeEventListener("resize", again);
      window.removeEventListener(UI_SCALE_EVENT, again);
      observer?.disconnect();
    };
  }, [target, rowRef]);
  useLayoutEffect(() => {
    if (target) recomputeRef.current();
  }, [target, fontSize, terminalId, ratio, live]);

  // The terminals refit once per change of mode or width (and when a file opens or closes).
  const fitKey = fit.mode === "side" ? `side:${fit.width}` : "overlay";
  useEffect(() => {
    const id = window.requestAnimationFrame(() => refit.current?.());
    return () => window.cancelAnimationFrame(id);
  }, [target, fitKey]);

  if (!target) return null;
  const shown = fit.mode === "side" && available > 0 ? 1 - fit.width / available : 1 - (live ?? ratio);
  const divider: PreviewDivider = {
    value: shown,
    min: 1 - MAX_PREVIEW_RATIO,
    max: 1 - MIN_PREVIEW_RATIO,
    defaultValue: 1 - DEFAULT_PREVIEW_RATIO,
    measure: (x) => {
      const rect = rowRef.current?.getBoundingClientRect();
      return rect && available > 0 ? (x - rect.left) / available : null;
    },
    onLive: (value) => setLive(value === null ? null : 1 - value),
    onCommit: (value) => {
      setRatio(1 - value);
      savePreviewRatio(windowId, 1 - value);
    },
  };
  return <PreviewPane target={target} onClose={() => setTarget(null)} fit={fit} divider={divider} />;
}
