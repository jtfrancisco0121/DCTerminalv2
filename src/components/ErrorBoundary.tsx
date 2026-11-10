import { Component, createRef, type ErrorInfo, type ReactNode } from "react";
import { errorText, InlineNotice } from "./InlineNotice";

export type BoundaryKind = "page" | "dialog" | "window";

type Props = {
  kind: BoundaryKind;
  /** Closes the surface this boundary guards (the window boundary has none). */
  onClose?: () => void;
  children: ReactNode;
};

type State = { error: Error | null; stack: string };

const TITLES: Record<BoundaryKind, string> = {
  page: "This page failed",
  dialog: "This dialog failed",
  window: "Something failed in this window",
};

/**
 * Contains a render error to the Page or Dialog it wraps, so the terminals and the rest of the window keep running.
 * App.tsx wraps each Page and Dialog call site; main.tsx adds one `window` boundary as the last resort. Never wrap
 * the terminal tree: that would unmount a running shell on an error elsewhere.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, stack: "" };
  private closeButton = createRef<HTMLButtonElement>();

  static getDerivedStateFromError(error: unknown): Partial<State> {
    return { error: error instanceof Error ? error : new Error(errorText(error)) };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ stack: info.componentStack ?? "" });
    console.error("dcv2 surface failed", error, info.componentStack);
  }

  componentDidUpdate() {
    if (this.state.error && !this.closeButton.current?.contains(document.activeElement)) this.closeButton.current?.focus();
  }

  componentDidMount() {
    this.closeButton.current?.focus();
  }

  private copy = () => {
    const { error, stack } = this.state;
    const text = `${error?.message ?? ""}\n${stack}`.trim();
    void navigator.clipboard?.writeText(text).catch(() => {});
  };

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const { kind, onClose } = this.props;
    return (
      <div className={`error-boundary error-boundary-${kind}`}>
        <InlineNotice
          tone="error"
          action={
            <>
              <button type="button" className="secondary-button" onClick={this.copy}>
                Copy error
              </button>
              {onClose && (
                <button type="button" ref={this.closeButton} className="primary-button" onClick={onClose}>
                  Close
                </button>
              )}
            </>
          }
        >
          <strong>{TITLES[kind]}</strong>
          <br />
          {errorText(error)}
          <br />
          The rest of the window is still running.
        </InlineNotice>
      </div>
    );
  }
}
