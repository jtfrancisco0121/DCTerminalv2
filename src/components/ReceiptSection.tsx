import type { PrView } from "../run/api";

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The trace-link row of this run: its label (before the link), or null for any other row. */
function traceRow(row: string, runId: string): { label: string } | null {
  const link = new RegExp(`\\[?dcterminalv2://run/${escape(runId)}(?![A-Za-z0-9_.-])`).exec(row);
  if (!link) return null;
  const label = row.slice(0, link.index).replace(/[:\s]+$/, "");
  return label ? { label } : null;
}

/**
 * A PR's Receipt in the run panel, read only: the rows are the `- ` lines of the Receipt block of the PR body, shown as plain
 * text (never HTML). The row with this run's trace link is only its label, as a button that opens the Trace (the link text adds nothing in the app).
 */
export function ReceiptSection({ pr, runId, onOpenTrace }: { pr: PrView; runId: string; onOpenTrace?: () => void }) {
  const receipt = pr.receipt;
  if (!receipt) return null;
  return (
    <details className="run-pr-receipt" data-testid={`receipt-${pr.id}`}>
      <summary aria-label={`Receipt for ${pr.id}`}>Receipt</summary>
      {receipt.state === "rows" && (
        <ul className="qa-criteria">
          {receipt.rows.map((row, at) => {
            const trace = onOpenTrace ? traceRow(row, runId) : null;
            return (
              <li key={at}>
                {trace ? (
                  <button type="button" className="link-button" onClick={onOpenTrace}>
                    {trace.label}
                  </button>
                ) : (
                  row
                )}
              </li>
            );
          })}
        </ul>
      )}
      {receipt.state === "off" && <p className="hint">Receipts are off for this repo</p>}
      {receipt.state === "none" && <p className="hint">No receipt in the PR body yet.</p>}
      {receipt.state === "unreadable" && <p className="hint">Receipt unreadable: {pr.id}&apos;s body could not be read.</p>}
    </details>
  );
}
