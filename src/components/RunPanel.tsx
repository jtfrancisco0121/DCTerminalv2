import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  readRun,
  readRunFile,
  removeRunWorktrees,
  rerunChecks,
  runWorktrees,
  type PrView,
  type RemoveResult,
  type RunView,
  type WorktreeItem,
} from "../run/api";
import {
  checkMark,
  checkSignature,
  prStateText,
  reviewText,
  runFinished,
  safeUrl,
  stageText,
  storiesText,
  wantsAppChecks,
} from "../run/words";
import { ClickUpRunSection } from "../clickup/ClickUpRunSection";
import { Dialog } from "./Dialog";
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
};

function PrRow({ pr, onOpenFile, onOpenUrl }: { pr: PrView } & Pick<ViewProps, "onOpenFile" | "onOpenUrl">) {
  const url = safeUrl(pr.prUrl);
  const stories = storiesText(pr);
  const review = reviewText(pr);
  const gate = pr.gateStatus;
  return (
    <li className="run-pr" data-testid={`pr-${pr.id}`}>
      <div className="run-pr-head">
        <strong>{pr.id}</strong>
        {pr.repo && <span className="run-pr-repo">{pr.repo}</span>}
        <span className={`run-pr-state run-pr-state-${pr.state ?? "unknown"}`}>{prStateText(pr.state)}</span>
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
          Open PR
        </button>
      )}
    </li>
  );
}

/** The progress panel for one run, drawn from a read of its folder. */
export function RunPanelView({ view, checking, onRerunChecks, onOpenFile, onOpenUrl, onCleanUp, extra }: ViewProps) {
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
      {!view.hasStatus && !view.statusProblem && <p className="hint">Claude is starting the run in the tab.</p>}
      {view.waitingFor && (
        <p className="run-waiting" role="status">
          Claude is asking you in the tab.
        </p>
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
      {view.prs.length > 0 && (
        <ul className="run-prs">
          {view.prs.map((pr) => (
            <PrRow key={pr.id} pr={pr} onOpenFile={onOpenFile} onOpenUrl={onOpenUrl} />
          ))}
        </ul>
      )}
      {view.shipNote && <p className="hint">{view.shipNote}</p>}
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
export function RunPanel({ runDir }: { runDir: string }) {
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
    readRunFile(runDir, path).then(
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
