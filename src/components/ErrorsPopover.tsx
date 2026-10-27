import { useEffect, useRef } from "react";
import { clearUiErrors, type UiError } from "../uiErrors";
import { localTime } from "../run/words";

type Props = {
  errors: readonly UiError[];
  onClose: () => void;
};

/** The errors this window caught (uiErrors.ts), newest first, at most 50, times in local time. */
export function ErrorsPopover({ errors, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const newestFirst = errors.slice(-50).reverse();

  useEffect(() => {
    ref.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      // The count button toggles the popover itself.
      if (target && !ref.current?.contains(target) && !(target as Element).closest?.(".error-count")) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
    };
  }, [onClose]);

  return (
    <div ref={ref} className="errors-popover" role="dialog" aria-label="Recent errors" tabIndex={-1}>
      <div className="errors-popover-head">
        <strong>Recent errors</strong>
        <button
          type="button"
          className="link-button"
          onClick={() => {
            clearUiErrors();
            onClose();
          }}
        >
          Clear
        </button>
      </div>
      {newestFirst.length === 0 && <p className="hint">No errors.</p>}
      <ul className="errors-list">
        {newestFirst.map((error, index) => {
          const time = localTime(error.at);
          return (
            <li key={`${error.at}-${index}`}>
              <span className="errors-source" title={time?.title}>
                {time?.short ?? ""} · {error.source}
              </span>
              <span className="errors-message">{error.message}</span>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
