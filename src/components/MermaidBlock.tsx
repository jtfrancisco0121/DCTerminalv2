import DOMPurify from "dompurify";
import { useEffect, useRef, useState } from "react";
import { THEMES } from "../theme";

let counter = 0;

/** The app theme's colours for Mermaid's "base" theme, read from the CSS variables; empty when they are not set (tests). */
function diagramTheme() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string) => css.getPropertyValue(name).trim();
  const dark = (THEMES.find((t) => t.id === document.documentElement.dataset.theme)?.scheme ?? "dark") === "dark";
  const [bg, node, border, fg, muted, accent] = ["--bg", "--bg-elev", "--fg-subtle", "--fg", "--fg-muted", "--accent-fg"].map(v);
  // Mermaid works the colours out (darker borders, contrast text), so it needs plain values, never var().
  if (![bg, node, border, fg, muted, accent].every((c) => /^#[0-9a-f]{3,8}$/i.test(c))) return { theme: dark ? "dark" : "default" } as const;
  return {
    theme: "base",
    themeVariables: {
      darkMode: dark,
      background: bg,
      mainBkg: node,
      primaryColor: node,
      primaryBorderColor: border,
      primaryTextColor: fg,
      secondaryColor: node,
      tertiaryColor: bg,
      lineColor: muted,
      textColor: fg,
      clusterBkg: bg,
      clusterBorder: border,
      edgeLabelBackground: bg,
      noteBkgColor: node,
      noteTextColor: fg,
      actorBkg: node,
      actorBorder: accent,
      actorTextColor: fg,
      signalColor: muted,
      signalTextColor: fg,
      fontSize: "13px",
    },
  } as const;
}

/** Changes whenever the app theme does, so diagrams are drawn again in the new colours. */
function useThemeKey() {
  const [key, setKey] = useState(() => document.documentElement.dataset.theme ?? "");
  useEffect(() => {
    const observer = new MutationObserver(() => setKey(document.documentElement.dataset.theme ?? ""));
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
    return () => observer.disconnect();
  }, []);
  return key;
}

/** The one place HTML is inserted: a Mermaid diagram's SVG, rendered in strict mode and run through DOMPurify's SVG profile. */
export function MermaidBlock({ source }: { source: string }) {
  const host = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const themeKey = useThemeKey();
  useEffect(() => {
    let live = true;
    setError(null);
    (async () => {
      const mermaid = (await import("mermaid")).default;
      // Labels as SVG <text>: HTML labels live in <foreignObject>, which the sanitiser removes, leaving empty boxes.
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict", htmlLabels: false, flowchart: { htmlLabels: false }, ...diagramTheme() });
      counter += 1;
      const { svg } = await mermaid.render(`dcv2-mermaid-${counter}`, source);
      const clean = DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true } });
      if (live && host.current) host.current.innerHTML = clean;
    })().catch((err: unknown) => {
      if (live) setError(err instanceof Error ? err.message : String(err));
    });
    return () => {
      live = false;
    };
  }, [source, themeKey]);
  if (error) {
    return (
      <div className="preview-mermaid-error" role="note">
        <p className="hint">The diagram could not be drawn ({error.split("\n")[0]}). Its source:</p>
        <pre>{source}</pre>
      </div>
    );
  }
  return <div ref={host} className="preview-mermaid" data-testid="mermaid" />;
}
