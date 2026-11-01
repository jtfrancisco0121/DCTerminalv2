import { openUrl } from "@tauri-apps/plugin-opener";
import { Children, isValidElement, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { previewFile, previewStat, type Preview } from "../preview/api";
import { PREVIEW_EVENT, resolveRelative, slug, type PreviewTarget } from "../preview/bus";
import { MermaidBlock } from "./MermaidBlock";
import { baseName } from "../paths";
import { errorText } from "./InlineNotice";
import { reportError } from "../uiErrors";

const POLL_MS = 1000;

type View =
  | { kind: "loading" }
  | { kind: "refused"; reason: string }
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

/** The preview beside the terminal: one file, re-read within about a second of a change, scroll kept. */
export function PreviewPane({ target, onClose }: { target: PreviewTarget; onClose: () => void }) {
  const [path, setPath] = useState(target.path);
  const [title, setTitle] = useState(target.title);
  const [view, setView] = useState<View>({ kind: "loading" });
  const scroller = useRef<HTMLDivElement>(null);
  const seen = useRef(0);

  useEffect(() => {
    setPath(target.path);
    setTitle(target.title);
  }, [target]);

  const load = useCallback(
    (keepScroll: boolean) => {
      const top = scroller.current?.scrollTop ?? 0;
      previewFile(path).then(
        (preview) => {
          seen.current = preview.mtimeMs;
          setView({ kind: "ok", preview });
          if (keepScroll) window.requestAnimationFrame(() => scroller.current && (scroller.current.scrollTop = top));
        },
        (err: unknown) => setView({ kind: "refused", reason: errorText(err) }),
      );
    },
    [path],
  );

  useEffect(() => {
    setView({ kind: "loading" });
    seen.current = 0;
    load(false);
  }, [load]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === "hidden") return;
      previewStat(path).then(
        (stat) => {
          if (!stat.exists) setView((v) => (v.kind === "removed" ? v : { kind: "removed" }));
          else if (stat.mtimeMs !== seen.current) load(true);
        },
        () => undefined,
      );
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [path, load]);

  const name = baseName(path) || path;
  return (
    <aside className="preview-pane" aria-label={`Preview: ${title}`} data-testid="preview-pane">
      <header className="preview-head">
        <strong title={path}>{title === name ? name : `${title} · ${name}`}</strong>
        <button type="button" className="link-button" onClick={onClose} aria-label="Close preview">
          Close
        </button>
      </header>
      <div ref={scroller} className="preview-body">
        {view.kind === "loading" && <p className="hint">Loading…</p>}
        {view.kind === "refused" && (
          <p className="hint warn" role="alert" data-testid="preview-refused">
            This file cannot be previewed: {view.reason}.
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
            <Markdown text={view.preview.text} from={view.preview.path} onNavigate={setPath} />
          </>
        )}
      </div>
    </aside>
  );
}

/** Between the terminal and the run panel: shows the file `openPreview` asked for. */
export function PreviewHost({ onLayoutChange }: { onLayoutChange?: () => void }) {
  const [target, setTarget] = useState<PreviewTarget | null>(null);
  useEffect(() => {
    const on = (event: Event) => setTarget((event as CustomEvent<PreviewTarget>).detail);
    window.addEventListener(PREVIEW_EVENT, on);
    return () => window.removeEventListener(PREVIEW_EVENT, on);
  }, []);
  const refit = useRef(onLayoutChange);
  refit.current = onLayoutChange;
  useEffect(() => {
    const id = window.requestAnimationFrame(() => refit.current?.());
    return () => window.cancelAnimationFrame(id);
  }, [target]);
  return target ? <PreviewPane target={target} onClose={() => setTarget(null)} /> : null;
}
