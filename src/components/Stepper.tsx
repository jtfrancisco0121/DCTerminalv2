import { runSteps, type RunStep, type Tone } from "../run/words";
import { CheckIcon, CrossIcon } from "./icons";

type Props = {
  stage: string | null;
  waitingFor: string | null;
  prs: { done: number; total: number };
  /** meta.json's run mode (`epic-plan` has three steps). */
  mode?: string | null;
  /** For a failed run: the last step it reached. */
  reached?: string | null;
  /** `compact`: one line, the current step named; `vertical`: dots only (the collapsed rail). */
  variant?: "default" | "compact" | "vertical";
};

const DOT_TONE: Record<RunStep["status"], Tone> = {
  done: "success",
  current: "running",
  todo: "muted",
  waiting: "you",
};

function stepTitle(step: RunStep): string {
  return `${step.label}${step.you ? " (your step)" : ""}${step.status === "waiting" ? ": waiting for you" : ""}`;
}

/**
 * Where a run is: its steps in order, the current one marked. Approval steps
 * are the user's and say so in words ("(you)"), and the one being waited at is
 * a ringed dot with "waiting for you" in its tooltip, so it never relies on hue alone. Null when the stage is not a
 * step (the stage sentence stands alone).
 */
export function Stepper({ stage, waitingFor, prs, mode, reached, variant = "default" }: Props) {
  const result = runSteps(stage, waitingFor, prs, { mode, reached });
  if (!result) return null;
  const dotsOnly = variant === "vertical";
  return (
    <ol className={`stepper${variant === "default" ? "" : ` stepper-${variant}`}`} aria-label="Run steps">
      {result.steps.map((step) => {
        const live = step.status === "current" || step.status === "waiting";
        const title = stepTitle(step);
        return (
          <li
            key={step.key}
            className={`step step-${step.status}${step.you ? " step-you" : ""}`}
            title={title}
            aria-current={live ? "step" : undefined}
            aria-label={dotsOnly ? title : undefined}
          >
            <span className={`step-dot tone-${DOT_TONE[step.status]}`} aria-hidden="true">
              {step.status === "done" && <CheckIcon size={12} />}
            </span>
            {!dotsOnly && (variant === "default" || live) && (
              <>
                <span className="step-label">{step.label}</span>
                {step.you && <span className="step-mark"> (you)</span>}
                {step.extra && <span className="step-extra">{step.extra}</span>}
              </>
            )}
          </li>
        );
      })}
      {result.failed && (
        <li className="step step-failed" title="Failed" aria-label={dotsOnly ? "Failed" : undefined}>
          <span className="step-dot tone-danger" aria-hidden="true">
            <CrossIcon size={12} />
          </span>
          {!dotsOnly && <span className="step-label">Failed</span>}
        </li>
      )}
    </ol>
  );
}
