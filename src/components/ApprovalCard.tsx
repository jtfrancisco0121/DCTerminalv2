import type { RunView } from "../run/api";
import {
  appCheckText,
  approvalTitle,
  checkTone,
  gateTone,
  isKnownApproval,
  planReviewTone,
  prStateText,
  prStateTone,
  reviewTone,
  riskTone,
} from "../run/words";
import { Pill } from "./Pill";

type Props = {
  view: RunView;
  /** The stop being waited at (status.json `waitingFor`). */
  waitingFor: string;
  onReadPlan: () => void;
  onGoToPrompt: () => void;
};

/**
 * What Claude is asking at Stop 1 and Stop 2, so the answer can be given
 * from facts: the PR count, each PR's gate, the app's own check and open
 * findings, and the plan review. Information only: there is no approve
 * button here, because the app never types into the tab. The answer is given
 * in the tab ("Go to prompt").
 */
export function ApprovalCard({ view, waitingFor, onReadPlan, onGoToPrompt }: Props) {
  const known = isKnownApproval(waitingFor);
  return (
    <section className="approval-card" role="status" aria-label="Approval" data-testid="approval-card">
      <h3 data-testid="approval-title">{approvalTitle(waitingFor)}</h3>
      {known && (
        <>
          <p className="hint" data-testid="approval-prs">
            {view.prs.length} {view.prs.length === 1 ? "PR" : "PRs"}
          </p>
          <div className="approval-badges">
            {view.plan?.reviewRound != null && (
              <Pill tone={planReviewTone(view.plan.openFindings ?? 0)} testId="approval-plan-badge">
                Plan review: round {view.plan.reviewRound} · {view.plan.openFindings ?? 0} open
              </Pill>
            )}
            {view.plan?.riskVerdict != null && (
              <Pill tone={riskTone(view.plan.riskVerdict)} testId="approval-risk-badge">
                Risk: {view.plan.riskVerdict} · {view.plan.riskOpenFindings ?? 0} open
              </Pill>
            )}
          </div>
          {view.prs.length > 0 && (
            <table className="approval-prs">
              <thead>
                <tr>
                  <th>PR</th>
                  <th>State</th>
                  <th>Gate</th>
                  <th>Check (app)</th>
                  <th>Open findings</th>
                </tr>
              </thead>
              <tbody>
                {view.prs.map((pr) => (
                  <tr key={pr.id} data-testid={`approval-${pr.id}`}>
                    <td>{pr.id}</td>
                    <td>
                      <Pill tone={prStateTone(pr.state)}>{prStateText(pr.state)}</Pill>
                    </td>
                    <td>
                      {pr.gateStatus && pr.gateStatus.kind !== "none" ? (
                        <Pill tone={gateTone(pr.gateStatus.kind)}>{pr.gateStatus.text}</Pill>
                      ) : (
                        "–"
                      )}
                    </td>
                    <td>
                      <Pill tone={pr.checkKnown ? checkTone(pr.appCheck?.result) : "muted"}>{appCheckText(pr)}</Pill>
                    </td>
                    <td>
                      <Pill tone={reviewTone(pr.review)}>{pr.review.openFindings ?? 0}</Pill>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </>
      )}
      <div className="button-row">
        {known && (
          <button type="button" className="secondary-button" onClick={onReadPlan}>
            Read plan
          </button>
        )}
        <button type="button" className="primary-button" onClick={onGoToPrompt}>
          Go to prompt
        </button>
      </div>
      <p className="hint">Answer in the tab: the app never types into it.</p>
    </section>
  );
}
