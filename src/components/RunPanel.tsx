import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  prWorktreeAction,
  prWorktreeChecks,
  readRun,
  readRunFile,
  removeRunWorktrees,
  rerunChecks,
  runWorktrees,
  slopCheck,
  type BmadView,
  type LogLine,
  type PrView,
  type PrWorktreeAction,
  type WorktreeCheck,
  type RemoveResult,
  type RunView,
  type SlopReport,
  type WorktreeItem,
} from "../run/api";
import {
  checkMark,
  checkSignature,
  checkTone,
  ciChip,
  draftBadge,
  gateTone,
  groupTimeline,
  localTime,
  prStateText,
  prsDone,
  prStateTone,
  reachedStep,
  reviewText,
  reviewTone,
  runFinished,
  safeUrl,
  stageText,
  storiesText,
  wantsAppChecks,
  type TimelineGroup,
} from "../run/words";
import { ClickUpRunSection } from "../clickup/ClickUpRunSection";
import { FleetTimeline } from "./FleetTimeline";
import { Pill } from "./Pill";
import { Stepper } from "./Stepper";
import { checkLogPr, readCheckLog } from "../run/observe";
import { RUN_STATE_EVENT, usePushRefresh } from "../run/events";
import { AWAY_ROWS, useAwayNotice } from "../run/away";
import { Dialog } from "./Dialog";
import { openPreview } from "../preview/bus";
import { RunPanelShell } from "./RunPanelShell";
import { ApprovalCard } from "./ApprovalCard";
import { baseName } from "../paths";
import { ObserveSection } from "./ObserveSection";
import { errorText } from "./InlineNotice";
import { reportError } from "../uiErrors";

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
  /** Each PR's worktree check (undefined until it is known). */
  worktrees?: Readonly<Record<string, WorktreeCheck>>;
  onPrAction?: (prId: string, action: PrWorktreeAction) => void;
  onCopyBranch?: (branch: string) => void;
  /** The slop check of one PR (the Slop check action). Missing: the action is not offered. */
  onSlopCheck?: (prId: string) => Promise<SlopReport>;
  /** "While you were away": the timeline lines since the panel was last seen, with its Dismiss. */
  away?: { lines: LogLine[]; dismiss: () => void } | null;
};

type PrActionProps = Pick<ViewProps, "worktrees" | "onPrAction" | "onCopyBranch">;

/**
 * What changes when a PR of the run merges: each PR's GitHub state and the
 * cleanup outcomes (`PR1:MERGED:cleaned|PR2:OPEN:`). A merge changes neither
 * status.json's stage nor `updatedAt`, so the ClickUp section reloads on this.
 */
export function mergeKey(view: RunView): string {
  const cleaned = new Map((view.cleanup ?? []).map((rec) => [rec.prId, rec.retry ? "" : rec.outcome]));
  return view.prs.map((pr) => `${pr.id}:${pr.prState ?? ""}:${cleaned.get(pr.id) ?? ""}`).join("|");
}

/** "While you were away": up to 10 rows (local times), "+n more", Dismiss. */
function AwayNotice({ lines, onDismiss }: { lines: LogLine[]; onDismiss: () => void }) {
  const shown = lines.slice(-AWAY_ROWS);
  const more = lines.length - shown.length;
  return (
    <section className="away-notice" aria-label="While you were away" data-testid="away-notice">
      <h3>While you were away</h3>
      <ul>
        {shown.map((line, index) => (
          <li key={`${line.time ?? ""}-${index}`}>
            {line.time && <TimelineTime time={line.time} />} {line.text}
          </li>
        ))}
      </ul>
      {more > 0 && <p className="hint">+{more} more</p>}
      <button type="button" className="secondary-button" onClick={onDismiss}>
        Dismiss
      </button>
    </section>
  );
}

/** One timeline time in the local zone; the title holds the full time and the UTC value. */
function TimelineTime({ time }: { time: string }) {
  const local = localTime(time);
  return (
    <span className="run-time" title={local?.title ?? time}>
      {local?.short ?? time.slice(11, 16)}
    </span>
  );
}

/** One stage's lines. It is open or closed only when it first mounts, so the user's choice survives a poll. */
function TimelineGroupView({ group, openAtMount }: { group: TimelineGroup; openAtMount: boolean }) {
  const [open] = useState(openAtMount);
  return (
    <details className="timeline-group" open={open}>
      <summary>
        {group.label} · {group.lines.length} {group.lines.length === 1 ? "line" : "lines"}
      </summary>
      <ol>
        {group.lines.map((line, at) => (
          <li key={`${line.time ?? ""}-${at}`}>
            {line.time && <TimelineTime time={line.time} />} {line.text}
          </li>
        ))}
      </ol>
    </details>
  );
}

/** The Slop check action of one PR and its results, grouped by file; a click on a finding copies `file:line`. */
export function SlopCheck({ prId, onSlopCheck }: { prId: string; onSlopCheck: (prId: string) => Promise<SlopReport> }) {
  const [state, setState] = useState<{ busy: boolean; report: SlopReport | null; error: string | null }>({
    busy: false,
    report: null,
    error: null,
  });
  const [copied, setCopied] = useState<string | null>(null);
  const run = () => {
    setState({ busy: true, report: null, error: null });
    onSlopCheck(prId).then(
      (report) => setState({ busy: false, report, error: null }),
      (err: unknown) => setState({ busy: false, report: null, error: errorText(err) }),
    );
  };
  const copy = (path: string, line: number) => {
    const where = `${path}:${line}`;
    navigator.clipboard.writeText(where).then(
      () => setCopied(where),
      () => setCopied(null),
    );
  };
  const groups = new Map<string, SlopReport["findings"]>();
  for (const f of state.report?.findings ?? []) groups.set(f.path, [...(groups.get(f.path) ?? []), f]);
  return (
    <div className="run-slop" data-testid={`slop-${prId}`}>
      <button type="button" className="link-button" disabled={state.busy} onClick={run}>
        {state.busy ? "Checking for slop…" : "Slop check"}
      </button>
      {state.error && <p className="error">{state.error}</p>}
      {state.report?.state === "worktree-missing" && (
        <p className="hint" data-testid={`slop-missing-${prId}`}>
          The worktree is gone, so there is nothing to check{state.report.detail ? ` (${state.report.detail})` : ""}.
        </p>
      )}
      {state.report?.state === "no-rules" && <p className="hint">No slop patterns are installed.</p>}
      {state.report?.state === "ok" && state.report.total === 0 && (
        <p className="hint" data-testid={`slop-none-${prId}`}>
          No slop found in the lines this PR added.
        </p>
      )}
      {state.report?.state === "ok" && state.report.total > 0 && (
        <div data-testid={`slop-results-${prId}`}>
          <p className="hint">
            {state.report.total} finding{state.report.total === 1 ? "" : "s"}
            {state.report.total > state.report.findings.length ? ` (first ${state.report.findings.length} shown)` : ""}. Click a line to copy
            file:line.{state.report.detail ? ` ${state.report.detail}.` : ""}
          </p>
          {[...groups.entries()].map(([path, items]) => (
            <div key={path} className="run-slop-file">
              <strong>{path}</strong>
              <ul>
                {items.map((f) => (
                  <li key={`${f.line}-${f.rule}`}>
                    <button type="button" className="link-button" onClick={() => copy(f.path, f.line)}>
                      line {f.line}
                    </button>{" "}
                    {f.message} <span className="hint">[{f.rule}]</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {copied && <p className="hint" role="status">Copied {copied}</p>}
        </div>
      )}
    </div>
  );
}

function PrRow({
  pr,
  onOpenFile,
  onOpenUrl,
  worktrees,
  onPrAction,
  onCopyBranch,
  onSlopCheck,
}: { pr: PrView } & Pick<ViewProps, "onOpenFile" | "onOpenUrl" | "onSlopCheck"> & PrActionProps) {
  const wt = worktrees?.[pr.id];
  const why = !wt ? "Checking the worktree…" : wt.verified ? undefined : (wt.reason ?? "The worktree is not verified.");
  const act = (action: PrWorktreeAction, label: string) => (
    <button
      type="button"
      className="link-button"
      disabled={!wt?.verified || !onPrAction}
      title={why}
      onClick={() => onPrAction?.(pr.id, action)}
    >
      {label}
    </button>
  );
  const url = safeUrl(pr.prUrl);
  const stories = storiesText(pr);
  const review = reviewText(pr);
  const gate = pr.gateStatus;
  const badge = draftBadge(pr);
  const ci = ciChip(pr);
  const comments = (pr.comments ?? []).filter((c) => safeUrl(c.url));
  return (
    <li className="run-pr" data-testid={`pr-${pr.id}`}>
      <div className="run-pr-head">
        <strong>{pr.id}</strong>
        {pr.repo && <span className="run-pr-repo">{pr.repo}</span>}
        <Pill tone={prStateTone(pr.state)} className={`run-pr-state run-pr-state-${pr.state ?? "unknown"}`}>
          {prStateText(pr.state)}
        </Pill>
        {badge && (
          <Pill
            tone={badge === "ready" ? "success" : "muted"}
            className={`run-pr-badge run-pr-badge-${badge}`}
            testId={`badge-${pr.id}`}
          >
            {badge}
          </Pill>
        )}
      </div>
      {pr.title && <div className="run-pr-title">{pr.title}</div>}
      <div className="run-pr-facts">
        {stories && <span>{stories}</span>}
        <Pill tone={checkTone(pr.check.result)} title={pr.check.summary ?? undefined}>
          check (Claude): {checkMark(pr.check.result)} {pr.check.result ?? "not run"}
        </Pill>
        {!pr.checkKnown && <Pill tone="muted">check (app): no check known</Pill>}
        {pr.checkKnown && (
          <span className="run-pr-app-check">
            <Pill tone={checkTone(pr.appCheck?.result)} title={pr.appCheck?.detail ?? pr.appCommand ?? undefined}>
              check (app): {checkMark(pr.appCheck?.result)} {pr.appCheck?.result ?? "not run"}
              {pr.appCheckStale && " (code changed since)"}
            </Pill>
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
        {ci && (
          <Pill tone={ci.tone} className={`ci-chip ci-chip-${ci.tone}`} title={ci.title || undefined} testId={`ci-${pr.id}`}>
            {ci.text}
          </Pill>
        )}
        {pr.checkKnown && pr.check.command && pr.check.command !== pr.appCommand && (
          <span className="run-pr-note" data-testid={`check-differs-${pr.id}`}>
            Claude ran <code>{pr.check.command}</code>; the app runs <code>{pr.appCommand}</code>
          </span>
        )}
        {review && <Pill tone={reviewTone(pr.review)}>{review}</Pill>}
      </div>
      {gate && gate.kind !== "none" && (
        <Pill tone={gateTone(gate.kind)} className={`run-gate run-gate-${gate.kind}`} testId={`gate-${pr.id}`}>
          {gate.kind === "proven" ? "✓ " : ""}
          {gate.text}
        </Pill>
      )}
      {pr.note && <div className="run-pr-note">{pr.note}</div>}
      <div className="run-pr-actions" data-testid={`actions-${pr.id}`}>
        {pr.branch && (
          <button type="button" className="link-button" onClick={() => onCopyBranch?.(pr.branch!)}>
            Copy branch
          </button>
        )}
        {pr.worktree && act("reveal", "Reveal")}
        {pr.worktree && act("editor", "Open in editor")}
        {pr.worktree && act("shell", "Shell here")}
      </div>
      {pr.worktree && onSlopCheck && <SlopCheck prId={pr.id} onSlopCheck={onSlopCheck} />}
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
  const file = baseName(path) || path;
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
export function RunPanelView({ view, checking, onRerunChecks, onOpenFile, onOpenUrl, onCleanUp, session, extra, onGoToPrompt, worktrees, onPrAction, onCopyBranch, onSlopCheck, away }: ViewProps) {
  const timeline = groupTimeline(view.timeline);
  return (
    <aside className="run-panel" aria-label="Run progress">
      {away && <AwayNotice lines={away.lines} onDismiss={away.dismiss} />}
      <header className="run-panel-head">
        <div className="run-stage" data-testid="run-stage">
          {stageText(view.stage)}
        </div>
        <Stepper
          stage={view.stage}
          waitingFor={view.waitingFor}
          prs={{ done: prsDone(view.prs), total: view.prs.length }}
          mode={view.runMode}
          reached={reachedStep(view.timeline, view.runMode)}
        />
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
            <PrRow
              key={pr.id}
              pr={pr}
              onOpenFile={onOpenFile}
              onOpenUrl={onOpenUrl}
              worktrees={worktrees}
              onPrAction={onPrAction}
              onCopyBranch={onCopyBranch}
              onSlopCheck={onSlopCheck}
            />
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
          {timeline.map((group, at) => (
            <TimelineGroupView key={group.key} group={group} openAtMount={at === 0} />
          ))}
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
  windowId,
  onTabsChanged,
  onLayoutChange,
}: {
  runDir: string;
  tabId?: string;
  onResume?: () => void;
  /** Focus the tab's terminal (the approval card's Go to prompt). */
  onGoToPrompt?: () => void;
  /** The window a "Shell here" tab opens in. */
  windowId?: string;
  /** A tab was added (Shell here): the window reads its tabs again. */
  onTabsChanged?: () => void;
  /** The panel was resized or collapsed: refit the terminal beside it. */
  onLayoutChange?: () => void;
}) {
  const [view, setView] = useState<RunView | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);
  const [file, setFile] = useState<{ title: string; text: string } | null>(null);
  const [janitor, setJanitor] = useState<{ items: WorktreeItem[] | null; results: RemoveResult[] | null } | null>(
    null,
  );
  const autoChecked = useRef<string | null>(null);
  const [worktrees, setWorktrees] = useState<Record<string, WorktreeCheck>>();
  const away = useAwayNotice(windowId ?? "main", runDir, view?.timeline, view !== null);

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

  // The next read starts only after the last one answered; `run-state`
  // events refresh it at once and slow the polling down.
  usePushRefresh(RUN_STATE_EVENT, load, { fastMs: POLL_MS });

  // A failed `gh` call shows "CI unknown"; its reason is reported once per kind of failure.
  useEffect(() => {
    for (const pr of view?.prs ?? []) {
      if (pr.ciError) reportError(`CI of ${pr.id}`, pr.ciError, { once: `ci:${runDir}:${pr.id}:${pr.ciError}` });
    }
  }, [view, runDir]);

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

  // Which worktrees the backend would act on: read again when a PR's
  // worktree or state changes, not on every poll (it runs git).
  const worktreeKey = view ? view.prs.map((pr) => `${pr.id}|${pr.worktree ?? ""}|${pr.state ?? ""}`).join(";") : "";
  useEffect(() => {
    if (!worktreeKey) return;
    let current = true;
    prWorktreeChecks(runDir).then(
      (list) => current && setWorktrees(Object.fromEntries(list.map((item) => [item.prId, item]))),
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, [runDir, worktreeKey]);

  const prAction = (prId: string, action: PrWorktreeAction) => {
    prWorktreeAction(runDir, prId, action, windowId).then(
      () => {
        if (action === "shell") onTabsChanged?.();
      },
      (err: unknown) => setProblem(errorText(err)),
    );
  };

  const copyBranch = (branch: string) => {
    navigator.clipboard.writeText(branch).catch((err: unknown) => setProblem(errorText(err)));
  };

  // Alt+click keeps the old text dialog; a plain click on a Markdown file opens the preview beside the terminal.
  const altClick = useRef(false);
  const openFile = (path: string, title: string) => {
    if (!altClick.current && /\.md$/i.test(path) && !checkLogPr(path)) {
      openPreview({ path: `${runDir.replace(/\/$/, "")}/${path}`, title });
      return;
    }
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
      <RunPanelShell windowId={windowId ?? "main"} stageLabel="Reading the run" attention={false} onLayoutChange={onLayoutChange}>
        <aside className="run-panel" aria-label="Run progress">
          <p className="hint">{problem ?? "Reading the run…"}</p>
        </aside>
      </RunPanelShell>
    );
  }
  return (
    <>
      <RunPanelShell
        windowId={windowId ?? "main"}
        stageLabel={stageText(view.stage)}
        attention={Boolean(view.waitingFor)}
        stage={view.stage}
        waitingFor={view.waitingFor}
        prs={{ done: prsDone(view.prs), total: view.prs.length }}
        mode={view.runMode}
        reached={reachedStep(view.timeline, view.runMode)}
        onLayoutChange={onLayoutChange}
      >
      <div className="run-panel-clicks" onClickCapture={(event) => { altClick.current = event.altKey; }}>
      <RunPanelView
        view={view}
        checking={checking}
        onRerunChecks={rerun}
        onOpenFile={openFile}
        onOpenUrl={(url) => void openUrl(url).catch((err: unknown) => setProblem(errorText(err)))}
        onCleanUp={openJanitor}
        onGoToPrompt={onGoToPrompt}
        worktrees={worktrees}
        onPrAction={prAction}
        onCopyBranch={copyBranch}
        onSlopCheck={(prId) => slopCheck(runDir, prId)}
        away={away}
        session={tabId ? <ObserveSection tabId={tabId} onResume={onResume ?? (() => undefined)} /> : undefined}
        extra={
          <>
            <FleetTimeline runDir={runDir} live={!runFinished(view)} />
            <ClickUpRunSection runDir={runDir} stage={view.stage} updatedAt={view.updatedAt} mergeKey={mergeKey(view)} />
          </>
        }
      />
      </div>
      </RunPanelShell>
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
