/** Small inline SVG icons (no icon library). They inherit the text colour and are hidden from assistive tech: the words next to them carry the meaning. */

import type { ReactNode } from "react";

export type IconProps = { size?: number; className?: string };

const MIN_SIZE = 12;
const MAX_SIZE = 16;
const DEFAULT_SIZE = 14;

function clampSize(size: number | undefined): number {
  if (size == null || !Number.isFinite(size)) return DEFAULT_SIZE;
  return Math.min(MAX_SIZE, Math.max(MIN_SIZE, size));
}

function Icon({ size, className, children }: IconProps & { children: ReactNode }) {
  const px = clampSize(size);
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      aria-hidden="true"
      focusable="false"
      width={px}
      height={px}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {children}
    </svg>
  );
}

export function CheckIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M3 8.5l3.2 3.2L13 4.8" />
    </Icon>
  );
}

export function CrossIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M4 4l8 8M12 4l-8 8" />
    </Icon>
  );
}

export function ClockIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <circle cx="8" cy="8" r="5.5" />
      <path d="M8 4.8V8l2.2 1.4" />
    </Icon>
  );
}

export function SpinnerIcon({ className, ...rest }: IconProps) {
  return (
    <Icon {...rest} className={className ? `icon-spin ${className}` : "icon-spin"}>
      <path d="M8 2.5a5.5 5.5 0 1 0 5.5 5.5" />
    </Icon>
  );
}

export function BranchIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <circle cx="4.5" cy="3.5" r="1.5" />
      <circle cx="4.5" cy="12.5" r="1.5" />
      <circle cx="11.5" cy="5.5" r="1.5" />
      <path d="M4.5 5v6M11.5 7c0 2.5-3 2-7 4.2" />
    </Icon>
  );
}

export function PrIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <circle cx="4.5" cy="3.5" r="1.5" />
      <circle cx="4.5" cy="12.5" r="1.5" />
      <circle cx="11.5" cy="12.5" r="1.5" />
      <path d="M4.5 5v6M11.5 11V6.5a2 2 0 0 0-2-2H8M9.5 2.5l-2 2 2 2" />
    </Icon>
  );
}

export function GearIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <circle cx="8" cy="8" r="2.2" />
      <path d="M8 1.8v1.7M8 12.5v1.7M1.8 8h1.7M12.5 8h1.7M3.6 3.6l1.2 1.2M11.2 11.2l1.2 1.2M12.4 3.6l-1.2 1.2M4.8 11.2l-1.2 1.2" />
    </Icon>
  );
}

export function WarningIcon(props: IconProps) {
  return (
    <Icon {...props}>
      <path d="M8 2.2l6.2 11H1.8z" />
      <path d="M8 6.5v3M8 11.5v.1" />
    </Icon>
  );
}
