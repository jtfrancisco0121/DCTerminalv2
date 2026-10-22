import type { ReactNode } from "react";

export type NoticeTone = "info" | "warning" | "error";

type Props = {
  tone?: NoticeTone;
  children: ReactNode;
  /** An optional button beside the text (Retry, Open, ...). */
  action?: ReactNode;
  className?: string;
};

/** Plain text of an unknown thrown value. Never "[object Object]". */
export function errorText(err: unknown): string {
  if (err instanceof Error) return err.message || err.name;
  if (typeof err === "string") return err;
  if (err && typeof err === "object") {
    const message = (err as { message?: unknown }).message;
    if (typeof message === "string" && message) return message;
    try {
      return JSON.stringify(err);
    } catch {
      return String(err);
    }
  }
  return String(err);
}

/** A message that stays in the flow of the page. Errors announce themselves. */
export function InlineNotice({ tone = "info", children, action, className = "" }: Props) {
  return (
    <div
      className={`inline-notice inline-notice-${tone} ${className}`.trim()}
      role={tone === "error" ? "alert" : "status"}
    >
      <span className="inline-notice-text">{children}</span>
      {action && <span className="inline-notice-action">{action}</span>}
    </div>
  );
}
