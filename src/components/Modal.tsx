import { useEffect, useRef, type ReactNode } from "react";

interface Props {
  title: string;
  labelId: string;
  /** Esc and the backdrop only close when this is set; during a running cleanup it is not. */
  onClose: (() => void) | null;
  /** Changes whenever the dialog switches screen, so focus can be put back on a safe element. */
  phase: string;
  children: ReactNode;
}

const FOCUSABLE = 'button:not(:disabled), [href], input:not(:disabled), [tabindex]:not([tabindex="-1"])';

/**
 * Focus goes to the first element marked data-initial-focus (a Cancel/Close button), else to the dialog itself.
 * Never to the primary action, so a stray Enter cannot confirm anything.
 */
export function Modal({ title, labelId, onClose, phase, children }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    return () => previous?.focus();
  }, []);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    (el.querySelector<HTMLElement>("[data-initial-focus]") ?? el).focus();
  }, [phase]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      if (closeRef.current) {
        e.stopPropagation();
        closeRef.current();
      }
      return;
    }
    if (e.key !== "Tab" || !box.current) return;
    const items = [...box.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    if (items.length === 0) {
      e.preventDefault();
      return;
    }
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement;
    if (e.shiftKey && (active === first || active === box.current)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && active === last) {
      e.preventDefault();
      first.focus();
    } else if (!box.current.contains(active)) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose?.()}>
      <div
        ref={box}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={labelId}
        tabIndex={-1}
        onKeyDown={onKeyDown}
      >
        <h2 className="modal-title" id={labelId}>
          {title}
        </h2>
        {children}
      </div>
    </div>
  );
}
