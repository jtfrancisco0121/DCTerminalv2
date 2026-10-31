import type { ReactNode } from "react";
import type { Tone } from "../run/words";

/**
 * A small status label: the words carry the meaning, the tone class (App.css
 * `.tone-*`) colours it with the theme's tokens, and an optional icon sits in
 * front. The `you` tone differs from `warn` by shape (outlined, no fill).
 */
export function Pill({
  tone,
  children,
  title,
  testId,
  className,
  icon,
}: {
  tone: Tone;
  children: ReactNode;
  title?: string;
  testId?: string;
  className?: string;
  icon?: ReactNode;
}) {
  return (
    <span className={`pill tone-${tone}${className ? ` ${className}` : ""}`} title={title} data-testid={testId}>
      {icon}
      {children}
    </span>
  );
}
