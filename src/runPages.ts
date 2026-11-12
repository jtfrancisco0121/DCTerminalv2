import type { RunListItem } from "./run/api";

export type RunPageId = "all-runs" | "dashboard" | "analytics" | "trace-search" | "compare" | "web-qa";

export type RunPage = {
  id: RunPageId;
  /** The command palette entry. */
  label: string;
  /** The Runs popover's link. */
  short: string;
  /** The palette's second line. */
  detail: string;
  /** Linked from the Runs popover (the palette lists every page). */
  inPopover: boolean;
};

/** The run pages, in order: one list for the Runs popover and the command palette, so the two cannot drift. */
export const RUN_PAGES: ReadonlyArray<RunPage> = [
  { id: "all-runs", label: "All runs", short: "All runs", detail: "Every run, newest first", inPopover: true },
  { id: "dashboard", label: "Run dashboard", short: "Dashboard", detail: "Every unfinished run tab", inPopover: true },
  {
    id: "analytics",
    label: "Run analytics",
    short: "Analytics",
    detail: "Cost, duration and waiting over finished runs",
    inPopover: true,
  },
  {
    id: "trace-search",
    label: "Search traces",
    short: "Trace search",
    detail: "Find runs by what happened in them",
    inPopover: true,
  },
  {
    id: "compare",
    label: "Compare runs",
    short: "Compare",
    detail: "The two most recent finished runs of this folder",
    inPopover: true,
  },
  { id: "web-qa", label: "Web QA", short: "Web QA", detail: "Web QA passes and their results", inPopover: false },
];

/** The default pair for Compare: the two newest finished runs of `folder` (any folder when null); the list is newest first. */
export function compareDefaults(runs: ReadonlyArray<RunListItem>, folder: string | null): [string | null, string | null] {
  const finished = runs.filter(
    (item) =>
      (!folder || item.folder === folder) && ["done", "failed", "eval-done"].includes(item.record?.outcome ?? item.stage ?? ""),
  );
  return [finished[0]?.runDir ?? null, finished[1]?.runDir ?? null];
}
