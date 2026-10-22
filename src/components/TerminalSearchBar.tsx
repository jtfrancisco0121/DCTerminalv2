import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { ISearchOptions } from "@xterm/addon-search";

/** The parts of xterm's SearchAddon the bar uses (a fake in tests). */
export type TerminalSearchLike = {
  findNext(term: string, options?: ISearchOptions): boolean;
  findPrevious(term: string, options?: ISearchOptions): boolean;
  clearDecorations(): void;
  onDidChangeResults?: (
    listener: (event: { resultIndex: number; resultCount: number }) => void,
  ) => { dispose(): void };
};

type Props = {
  search: TerminalSearchLike;
  /** Called after decorations are cleared. The caller refocuses xterm. */
  onClose: () => void;
};

/** Match highlights from the theme tokens. The overview ruler colors are required by the addon. */
function searchDecorations() {
  const style = getComputedStyle(document.documentElement);
  const token = (name: string, fallback: string) =>
    style.getPropertyValue(name).trim() || fallback;
  const match = token("--search-match-border", "#d29922");
  const active = token("--search-active-border", "#ffa657");
  return {
    matchBackground: token("--search-match", "#d2992266"),
    matchBorder: match,
    matchOverviewRuler: match,
    activeMatchBackground: token("--search-active", "#f0883e"),
    activeMatchBorder: active,
    activeMatchColorOverviewRuler: active,
  };
}

/**
 * F5 terminal search, after ADE's TerminalSearch: search as you type,
 * Enter / Shift+Enter to step, Escape to close, case / word / regex toggles.
 */
export function TerminalSearchBar({ search, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [wholeWord, setWholeWord] = useState(false);
  const [regex, setRegex] = useState(false);
  const [found, setFound] = useState<boolean | null>(null);
  const [results, setResults] = useState<{ index: number; count: number } | null>(null);
  const [invalid, setInvalid] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const lastOptionsRef = useRef("");

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  useEffect(() => {
    const sub = search.onDidChangeResults?.((event) =>
      setResults({ index: event.resultIndex, count: event.resultCount }),
    );
    return () => sub?.dispose();
  }, [search]);

  const run = useCallback(
    (direction: "next" | "prev", incremental: boolean) => {
      if (!query) {
        search.clearDecorations();
        setFound(null);
        setResults(null);
        setInvalid(false);
        return;
      }
      const options: ISearchOptions = {
        caseSensitive,
        wholeWord,
        regex,
        incremental,
        decorations: searchDecorations(),
      };
      // addon-search keeps its last options and misses a change on the same
      // query; clearing first makes it recount (e.g. Match case toggled).
      const optionKey = `${caseSensitive}|${wholeWord}|${regex}`;
      if (lastOptionsRef.current !== optionKey) {
        lastOptionsRef.current = optionKey;
        search.clearDecorations();
      }
      try {
        const hit =
          direction === "next"
            ? search.findNext(query, options)
            : search.findPrevious(query, options);
        setFound(hit);
        setInvalid(false);
      } catch {
        setFound(false);
        setResults(null);
        setInvalid(true);
      }
    },
    [caseSensitive, query, regex, search, wholeWord],
  );

  // Search as you type, and again when an option changes.
  useEffect(() => {
    run("next", true);
  }, [run]);

  const close = () => {
    search.clearDecorations();
    onClose();
  };

  // On the root so Escape also closes from the toggle and nav buttons.
  const onRootKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    close();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      run(event.shiftKey ? "prev" : "next", false);
    }
  };

  let status = "";
  if (invalid) status = "Invalid pattern";
  else if (query && found === false) status = "No results";
  else if (query && results && results.count > 0) {
    status = results.index >= 0 ? `${results.index + 1} of ${results.count}` : `${results.count}`;
  } else if (query && found) status = "Found";

  const toggle = (label: string, text: string, on: boolean, set: (next: boolean) => void) => (
    <button
      type="button"
      className={on ? "search-toggle search-toggle-on" : "search-toggle"}
      aria-label={label}
      aria-pressed={on}
      title={label}
      onClick={() => {
        set(!on);
        inputRef.current?.focus();
      }}
    >
      {text}
    </button>
  );

  return (
    <div
      className="terminal-search"
      role="search"
      // While open, Escape closes the bar, not a running turn (this terminal
      // is also the shell pane under a chat tab).
      data-own-escape=""
      onKeyDown={onRootKeyDown}
    >
      <input
        ref={inputRef}
        type="search"
        className="text-input"
        aria-label="Search terminal"
        placeholder="Search terminal"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={onKeyDown}
      />
      <span className="search-status" aria-live="polite">
        {status}
      </span>
      {toggle("Match case", "Aa", caseSensitive, setCaseSensitive)}
      {toggle("Whole word", "W", wholeWord, setWholeWord)}
      {toggle("Regular expression", ".*", regex, setRegex)}
      <button
        type="button"
        className="search-nav"
        aria-label="Previous match"
        title="Previous match (Shift+Enter)"
        onClick={() => run("prev", false)}
      >
        ↑
      </button>
      <button
        type="button"
        className="search-nav"
        aria-label="Next match"
        title="Next match (Enter)"
        onClick={() => run("next", false)}
      >
        ↓
      </button>
      <button
        type="button"
        className="search-nav"
        aria-label="Close search"
        title="Close (Esc)"
        onClick={close}
      >
        ×
      </button>
    </div>
  );
}
