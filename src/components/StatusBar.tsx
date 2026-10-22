import type { ConfigDirInfo } from "../bridge";
import type { LimitLine } from "../usage/limits";

type Props = {
  /** This window's Claude account. Null until the window context loads. */
  account: { name: string; config: ConfigDirInfo } | null;
  /** The active tab's folder. */
  folder?: string | null;
  /** Claude 5h/7d line. Null hides it. */
  usage?: LimitLine | null;
  /** A short notice (a failed action), with its dismiss. */
  message?: { text: string; onDismiss: () => void } | null;
  onNewWindow: () => void;
  onSettings: () => void;
};

function folderName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * Why an account's folder will not work: one you set that does not exist.
 * The default `~/.claude` may not exist yet; Claude Code makes it.
 */
export function accountFolderWarning(config: ConfigDirInfo): string | null {
  if (config.exists || config.source === "default") return null;
  return `Claude config folder not found: ${config.path}`;
}

/**
 * One strip under the terminal: the window's account and its folder, the
 * active tab's folder, Claude usage, and the window and settings buttons.
 */
export function StatusBar({ account, folder = null, usage = null, message = null, onNewWindow, onSettings }: Props) {
  const warning = account ? accountFolderWarning(account.config) : null;
  return (
    <footer className="status-bar" aria-label="Status bar">
      {account && (
        <span
          className={`status-bar-item status-account${warning ? " status-bar-warn" : ""}`}
          title={warning ?? `Claude account for this window\n${account.config.path}`}
          data-testid="status-account"
        >
          {account.name} · {account.config.display}
          {warning && " · folder not found"}
        </span>
      )}
      {folder && (
        <span className="status-bar-item status-bar-folder" title={folder}>
          {folderName(folder)}
        </span>
      )}
      {usage && (
        <span
          className={`status-bar-item status-usage status-usage-${usage.tone}`}
          title={usage.title}
          data-testid="status-usage"
        >
          {usage.text}
        </span>
      )}
      {message && (
        <span className="status-bar-message status-bar-message-error" role="alert" title={message.text}>
          <span className="status-bar-message-text">{message.text}</span>
          <button
            type="button"
            className="status-bar-dismiss"
            aria-label={`Dismiss: ${message.text}`}
            onClick={message.onDismiss}
          >
            ×
          </button>
        </span>
      )}
      <span className="status-bar-spacer" />
      <button type="button" className="status-bar-button" onClick={onNewWindow} title="New window (⌘⇧N)">
        New window…
      </button>
      <button type="button" className="status-bar-button" aria-label="Settings" title="Settings (⌘,)" onClick={onSettings}>
        ⚙
      </button>
    </footer>
  );
}
