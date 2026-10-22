import { useEffect, useRef, type ReactNode } from "react";

type Props = {
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  className?: string;
};

/**
 * A modal panel over the window: Escape and the backdrop close it, focus
 * moves into it and goes back where it was when it closes.
 */
export function Dialog({ title, onClose, children, footer, className = "" }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const before = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>("input, select, button:not(.dialog-close)");
    (first ?? ref.current)?.focus();
    return () => before?.focus?.();
  }, []);

  return (
    <div className="dialog-backdrop" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div
        ref={ref}
        className={`dialog ${className}`.trim()}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }}
      >
        <div className="dialog-header">
          <h2 className="dialog-title">{title}</h2>
          <button type="button" className="ghost-button dialog-close" aria-label="Close" onClick={onClose}>
            ×
          </button>
        </div>
        <div className="dialog-body">{children}</div>
        {footer && <div className="dialog-footer">{footer}</div>}
      </div>
    </div>
  );
}
