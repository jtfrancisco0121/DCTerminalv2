import type { ReactNode } from "react";
import { shortcutLabel } from "../keyTable";
import type { Platform } from "../terminal/keys";

type Props = {
  /** The tab bar (absent with no tabs). */
  children?: ReactNode;
  /** The window's Claude account; the native title that showed it is hidden on macOS. */
  accountName?: string;
  platform: Platform;
  onOpenPalette: () => void;
};

/**
 * The top row of the window: tabs, an empty drag area, the account name and the
 * command-palette button. Only the spacer and the account label carry the drag
 * attribute; tabs and buttons never do, so they keep their clicks and reordering.
 * On macOS the native title bar is an overlay and the row leaves room for the traffic lights.
 */
export function TitleBar({ children, accountName, platform, onOpenPalette }: Props) {
  const account = accountName?.trim();
  return (
    <header className={`title-bar${platform === "mac" ? " title-bar-mac" : ""}`}>
      {children}
      <div className="title-bar-spacer" data-tauri-drag-region />
      {account && (
        <span className="title-bar-account" data-tauri-drag-region>
          {account}
        </span>
      )}
      <button type="button" className="title-bar-search" onClick={onOpenPalette} aria-label="Search commands">
        <span className="title-bar-search-hint">{shortcutLabel("palette", platform)}</span>
      </button>
    </header>
  );
}
