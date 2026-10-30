import DOMPurify from "dompurify";
import { useEffect, useRef, useState } from "react";

let counter = 0;

/** The one place HTML is inserted: a Mermaid diagram's SVG, rendered in strict mode and run through DOMPurify's SVG profile. */
export function MermaidBlock({ source }: { source: string }) {
  const host = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setError(null);
    (async () => {
      const mermaid = (await import("mermaid")).default;
      mermaid.initialize({ startOnLoad: false, securityLevel: "strict" });
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
  }, [source]);
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
