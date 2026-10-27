import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  readRun,
  readRunFile,
  removeRunWorktrees,
  rerunChecks,
  runWorktrees,
  type BmadView,
  type PrView,
  type RemoveResult,
  type RunView,
  type WorktreeItem,
} from "../run/api";
import {
  checkMark,
  checkSignature,
  draftBadge,
  prStateText,
  reviewText,
  runFinished,
  safeUrl,
  stageText,
  storiesText,
  wantsAppChecks,
} from "../run/words";
import { ClickUpRunSection } from "../clickup/ClickUpRunSection";
import { checkLogPr, readCheckLog } from "../run/observe";
import { Dialog } from "./Dialog";
import { ApprovalCard } from "./ApprovalCard";
import { ObserveSection } from "./ObserveSection";
import { errorText } from "./InlineNotice";

/** How often the panel reads the run folder again. */
const POLL_MS = 2000;

type ViewProps = {
  view: RunView;
  checking: boolean;
  onRerunChecks: () => void;
  onOpenFile: (path: string, title: string) => void;
  onOpenUrl: (url: string) => void;
  onCleanUp: () => void;
  /** Drawn before the timeline (the ClickUp section). */
  extra?: ReactNode;
  /** P3b: the tab's session (state, watchdog, context, cost). */
  session?: ReactNode;
  /** Focus the tab's terminal, where the question is asked. */
  onGoToPrompt?: () => void;
};

function PrRow({ pr, onOpenFile, onOpenUrl }: { pr: PrView } & Pick<ViewProps, "onOpenFile" | "onOpenUrl">) {
  const url = safeUrl(pr.prUrl);
  const stories = storiesText(pr);
  const review = reviewText(pr);
  const gate = pr.gateStatus;
  const badge = draftBadge(pr);
  const comments = (pr.comments ?? []).filter((c) => safeUrl(c.url));
  return (
    <li className="run-pr" data-testid={`pr-${pr.id}`}>
      <div className="run-pr-head">
        <strong>{pr.id}</strong>
        {pr.repo && <span className="run-pr-repo">{pr.repo}</span>}
        <span className={`run-pr-state run-pr-state-${pr.state ?? "unknown"}`}>{prStateText(pr.state)}</span>
        {badge && (
          <span className={`run-pr-badge run-pr-badge-${badge}`} data-testid={`badge-${pr.id}`}>
            {badge}
          </span>
        )}
      </div>
      {pr.title && <div className="run-pr-title">{pr.title}</div>}
      <div className="run-pr-facts">
        {stories && <span>{stories}</span>}
        <span title={pr.check.summary ?? undefined}>
          check (Claude): {checkMark(pr.check.result)} {pr.check.result ?? "not run"}
        </span>
        {!pr.checkKnown && <span>check (app): no check known</span>}
        {pr.checkKnown && (
          <span title={pr.appCheck?.detail ?? pr.appCommand ?? undefined}>
            check (app): {checkMark(pr.appCheck?.result)} {pr.appCheck?.result ?? "not run"}
            {pr.appCheckStale && " (code changed since)"}
            {pr.appCheck?.log && (
              <button
                type="button"
                className="link-button"
                onClick={() => onOpenFile(pr.appCheck!.log!, `${pr.id} app check`)}
              >
                output
              </button>
            )}
          </span>
        )}
        {pr.checkKnown && pr.check.command && pr.check.command !== pr.appCommand && (
          <span className="run-pr-note" data-testid={`check-differs-${pr.id}`}>
            Claude ran <code>{pr.check.command}</code>; the app runs <code>{pr.appCommand}</code>
          </span>
        )}
        {review && <span>{review}</span>}
      </div>
      {gate && gate.kind !== "none" && (
        <div className={`run-gate run-gate-${gate.kind}`} data-testid={`gate-${pr.id}`}>
          {gate.kind === "proven" ? "✓ " : ""}
          {gate.text}
        </div>
      )}
      {pr.note && <div className="run-pr-note">{pr.note}</div>}
      {url && (
        <button type="button" className="link-button" onClick={() => onOpenUrl(url)}>
          {badge === "draft" ? "Open draft PR" : "Open PR"}
        </button>
      )}
      {comments.length > 0 && (
        <div className="run-pr-comments" data-testid={`comments-${pr.id}`}>
          Review comments:
          {comments.map((c, at) => (
            <button key={c.url} type="button" className="link-button" onClick={() => onOpenUrl(c.url)}>
              round {c.round ?? at + 1}
            </button>
          ))}
        </div>
      )}
    </li>
  );
}

/** The BMAD documents are openable once the plan is being approved, and after (a failed run's too). */
const BMAD_DOCS_FROM = ["plan-approval", "building", "ship-approval", "shipping", "done", "failed"];

/** A short title for a BMAD document path (`bmad/output/<id>/spec-x/spec-x.md`). */
function bmadDocTitle(path: string): string {
  const parts = path.split("/");
  const file = parts[parts.length - 1] ?? path;
  if (file === "DESIGN.md") return "UX design";
  if (file === "EXPERIENCE.md") return "UX experience";
  const kind = file.split("-")[0];
  const names: Record<string, string> = { spec: "Spec", architecture: "Architecture", prd: "PRD", ux: "UX" };
  return names[kind] ?? file;
}

function BmadSection({
  bmad,
  stage,
  onOpenFile,
}: {
  bmad: BmadView;
  stage: string | null;
  onOpenFile: ViewProps["onOpenFile"];
}) {
  const showDocs = stage !== null && BMAD_DOCS_FROM.includes(stage) && bmad.documents.length > 0;
  return (
    <section className="run-bmad" aria-label="BMAD planning" data-testid="bmad">
      <p>
        <span className="run-pr-badge" data-testid="bmad-badge">
          {bmad.version ? `BMAD ${bmad.version}` : "BMAD"}
        </span>{" "}
        <span data-testid="bmad-size">
          {bmad.size
            ? `size ${bmad.size}${bmad.applied ? "" : ", not used"}`
            : bmad.mode === "on"
              ? "on"
              : "sizing the work"}
        </span>
      </p>
      {bmad.reasons.length > 0 && (
        <ul className="hint" data-testid="bmad-reasons">
          {bmad.reasons.map((reason) => (
            <li key={reason}>{reason}</li>
          ))}
        </ul>
      )}
      {showDocs && (
        <div className="run-bmad-docs" data-testid="bmad-docs">
          Documents:
          {bmad.documents.map((path) => (
            <button key={path} type="button" className="link-button" onClick={() => onOpenFile(path, bmadDocTitle(path))}>
              {bmadDocTitle(path)}
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

/** The progress panel for one run, drawn from a read of its folder. */
export function RunPanelView({ view, checking, onRerunChecks, onOpenFile, onOpenUrl, onCleanUp, session, extra, onGoToPrompt }: ViewProps) {
  const timeline = view.timeline.slice(-30).reverse();
  return (
    <aside className="run-panel" aria-label="Run progress">
      <header className="run-panel-head">
        <div className="run-stage" data-testid="run-stage">
          {stageText(view.stage)}
        </div>
        <div className="hint run-id">{view.runId}</div>
      </header>
      {view.statusProblem && <p className="hint warn">{view.statusProblem}</p>}
      {session}
      {!view.hasStatus && !view.statusProblem && <p className="hint">Claude is starting the run in the tab.</p>}
      {view.waitingFor && (
        <ApprovalCard
          view={view}
          waitingFor={view.waitingFor}
          onReadPlan={() => onOpenFile("plan.md", "Plan")}
          onGoToPrompt={onGoToPrompt ?? (() => undefined)}
        />
      )}
      {view.pausedReason && <p className="hint warn">Paused: {view.pausedReason}</p>}
      {view.error && <p className="error">{view.error}</p>}
      {view.plan && (
        <p className="run-plan">
          Plan {view.plan.approved ? "✓ approved" : view.plan.verdict ? `review: ${view.plan.verdict}` : "being written"}
          <button type="button" className="link-button" onClick={() => onOpenFile("plan.md", "Plan")}>
            read
          </button>
        </p>
      )}
      {view.bmad && view.bmad.mode !== "off" && (
        <BmadSection bmad={view.bmad} stage={view.stage} onOpenFile={onOpenFile} />
      )}
      {view.prs.length > 0 && (
        <ul className="run-prs">
          {view.prs.map((pr) => (
            <PrRow key={pr.id} pr={pr} onOpenFile={onOpenFile} onOpenUrl={onOpenUrl} />
          ))}
        </ul>
      )}
      {view.shipNote && <p className="hint">{view.shipNote}</p>}
      {(view.cleanup ?? []).length > 0 && (
        <section className="run-cleanup" aria-label="Cleanup" data-testid="cleanup">
          <h3>Cleaned up (merged PRs)</h3>
          <ul className="hint">
            {(view.cleanup ?? []).map((rec) => (
              <li key={rec.prId} data-testid={`cleanup-${rec.prId}`}>
                <strong>{rec.prId}</strong>{" "}
                {[rec.note, rec.worktree, rec.branch, rec.remote].filter((part) => part).join("; ")}
                {rec.retry && " (the app tries again later)"}
              </li>
            ))}
          </ul>
        </section>
      )}
      <div className="button-row">
        {view.prs.length > 0 && (
          <button type="button" className="secondary-button" disabled={checking} onClick={onRerunChecks}>
            {checking ? "Running checks…" : "Re-run checks"}
          </button>
        )}
        {view.hasReport && (
          <button type="button" className="secondary-button" onClick={() => onOpenFile("report.md", "Run report")}>
            Report
          </button>
        )}
        <button type="button" className="secondary-button" onClick={onCleanUp}>
          {runFinished(view) ? "Clean up worktrees" : "Stop and clean up…"}
        </button>
      </div>
      {extra}
      {timeline.length > 0 && (
        <section className="run-timeline" aria-label="Timeline">
          <h3>Timeline</h3>
          <ol>
            {timeline.map((line, at) => (
              <li key={`${line.time ?? ""}-${at}`}>
                {line.time && <span className="run-time">{line.time.slice(11, 16)}</span>} {line.text}
              </li>
            ))}
          </ol>
        </section>
      )}
    </aside>
  );
}

type JanitorProps = {
  items: WorktreeItem[] | null;
  results: RemoveResult[] | null;
  finished: boolean;
  onRemove: (prIds: string[]) => void;
  onClose: () => void;
};

/** R5: the run's worktrees, clean or not; removes only clean ones. */
export function JanitorDialog({ items, results, finished, onRemove, onClose }: JanitorProps) {
  const [confirm, setConfirm] = useState(false);
  const removable = (items ?? []).filter((item) => item.removable);
  return (
    <Dialog title="Worktrees of this run" onClose={onClose}>
      {!finished && (
        <p className="hint warn">
          The run is not finished. Stop Claude in its tab first (or close the tab); you can resume the run later.
        </p>
      )}
      {items === null && <p className="hint">Looking…</p>}
      {items?.length === 0 && <p className="hint">This run has no worktrees.</p>}
      {items && items.length > 0 && (
        <ul className="janitor-list">
          {items.map((item) => (
            <li key={`${item.prId}-${item.worktree}`}>
              <strong>{item.prId}</strong> <code>{item.worktree}</code>{" "}
              {item.problem
                ? `— ${item.problem}`
                : item.clean
                  ? item.disposableOnly
                    ? "— clean (build output and dependencies go with it)"
                    : "— clean"
                  : `— ${item.changedFiles} changed files, kept`}
            </li>
          ))}
        </ul>
      )}
      {results && (
        <ul className="janitor-results">
          {results.map((r) => (
            <li key={r.prId}>
              {r.prId}: {r.message}
            </li>
          ))}
        </ul>
      )}
      <div className="button-row">
        {removable.length > 0 && !confirm && (
          <button type="button" className="secondary-button" onClick={() => setConfirm(true)}>
            Remove {removable.length} clean {removable.length === 1 ? "worktree" : "worktrees"}
          </button>
        )}
        {confirm && (
          <>
            <span>Remove them? Branches are kept unless merged.</span>
            <button
              type="button"
              className="danger-button"
              onClick={() => {
                setConfirm(false);
                onRemove(removable.map((item) => item.prId));
              }}
            >
              Remove
            </button>
            <button type="button" className="secondary-button" onClick={() => setConfirm(false)}>
              Cancel
            </button>
          </>
        )}
      </div>
    </Dialog>
  );
}

export function TextDialog({ title, text, onClose }: { title: string; text: string; onClose: () => void }) {
  return (
    <Dialog title={title} onClose={onClose} className="text-dialog">
      <pre className="text-viewer">{text}</pre>
    </Dialog>
  );
}

/** The panel beside an Eagle-Eye tab: polls the run folder. */
export function RunPanel({
  runDir,
  tabId,
  onResume,
  onGoToPrompt,
}: {
  runDir: string;
  tabId?: string;
  onResume?: () => void;
  /** Focus the tab's terminal (the approval card's Go to prompt). */
  onGoToPrompt?: () => void;
}) {
  const [view, setView] = useState<RunView | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [file, setFile] = useState<{ title: string; text: string } | null>(null);
  const [janitor, setJanitor] = useState<{ items: WorktreeItem[] | null; results: RemoveResult[] | null } | null>(
    null,
  );
  const autoChecked = useRef<string | null>(null);

  const load = useCallback(
    () =>
      readRun(runDir).then(
        (next) => {
          setProblem(null);
          // A status.json that could not be read keeps the last good view.
          setView((last) =>
            next.statusProblem && last?.hasStatus ? { ...last, statusProblem: next.statusProblem } : next,
          );
        },
        (err: unknown) => setProblem(errorText(err)),
      ),
    [runDir],
  );

  // The next read starts only after the last one answered.
  useEffect(() => {
    let stopped = false;
    let timer: number | undefined;
    const tick = () => {
      void load().finally(() => {
        if (!stopped) timer = window.setTimeout(tick, POLL_MS);
      });
    };
    tick();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [load]);

  const rerun = useCallback(() => {
    setChecking(true);
    rerunChecks(runDir)
      .catch((err: unknown) => setProblem(errorText(err)))
      .finally(() => {
        setChecking(false);
        void load();
      });
  }, [runDir, load]);

  // R1: at stop #2 the app re-runs the checks on its own, again when a
  // worktree's HEAD moved since (once per set of HEADs).
  useEffect(() => {
    if (!view || checking || !wantsAppChecks(view)) return;
    const signature = checkSignature(view);
    if (autoChecked.current === signature) return;
    autoChecked.current = signature;
    rerun();
  }, [view, checking, rerun]);

  const openFile = (path: string, title: string) => {
    // A4: the app's check output is shown trimmed (ANSI, redraws, middle).
    const pr = checkLogPr(path);
    (pr ? readCheckLog(runDir, pr) : readRunFile(runDir, path)).then(
      (text) => setFile({ title, text }),
      (err: unknown) => setFile({ title, text: errorText(err) }),
    );
  };

  const openJanitor = () => {
    setJanitor({ items: null, results: null });
    runWorktrees(runDir).then(
      (items) => setJanitor({ items, results: null }),
      (err: unknown) => setProblem(errorText(err)),
    );
  };

  const remove = (prIds: string[]) => {
    removeRunWorktrees(runDir, prIds).then(
      (results) =>
        runWorktrees(runDir).then((items) => setJanitor({ items, results })),
      (err: unknown) => setProblem(errorText(err)),
    );
  };

  if (!view) {
    return (
      <aside className="run-panel" aria-label="Run progress">
        <p className="hint">{problem ?? "Reading the run…"}</p>
      </aside>
    );
  }
  return (
    <>
      <RunPanelView
        view={view}
        checking={checking}
        onRerunChecks={rerun}
        onOpenFile={openFile}
        onOpenUrl={(url) => void openUrl(url).catch((err: unknown) => setProblem(errorText(err)))}
        onCleanUp={openJanitor}
        onGoToPrompt={onGoToPrompt}
        session={tabId ? <ObserveSection tabId={tabId} onResume={onResume ?? (() => undefined)} /> : undefined}
        extra={<ClickUpRunSection runDir={runDir} stage={view.stage} updatedAt={view.updatedAt} />}
      />
      {problem && <p className="error run-problem">{problem}</p>}
      {file && <TextDialog title={file.title} text={file.text} onClose={() => setFile(null)} />}
      {janitor && (
        <JanitorDialog
          items={janitor.items}
          results={janitor.results}
          finished={runFinished(view)}
          onRemove={remove}
          onClose={() => setJanitor(null)}
        />
      )}
    </>
  );
}
