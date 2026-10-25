import { useEffect, useId, useRef, type ReactNode } from "react";
import { Icon } from "./Icon";

/**
 * Native modal <dialog>. Keep it mounted and drive `open`, so CSS can animate it out.
 * Esc and a click on the backdrop call onClose. Focus goes to [data-autofocus] or the first field.
 */
export function Dialog({ open, onClose, title, width = 420, children }: {
  open: boolean;
  onClose: () => void;
  title: string;
  width?: number;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const downOnBackdrop = useRef(false);
  const titleId = useId();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      d.showModal();
      // showModal would focus the header's close button; prefer the marked or first field.
      const first = d.querySelector<HTMLElement>("[data-autofocus]") ?? d.querySelector<HTMLElement>("input:not([disabled]), textarea");
      first?.focus();
    }
    if (!open && d.open) d.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      className="dlg"
      style={{ width }}
      aria-labelledby={titleId}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onPointerDown={(e) => (downOnBackdrop.current = e.target === e.currentTarget)}
      onClick={(e) => {
        if (downOnBackdrop.current && e.target === e.currentTarget) onClose();
      }}
    >
      <div className="flex items-center gap-3 pl-5 pr-1 h-12 border-b border-line">
        <h2 id={titleId} className="flex-1 text-[15px] font-semibold tracking-[-0.01em]">{title}</h2>
        <button type="button" className="wc w-10 h-10" aria-label="Close" onClick={onClose}>
          <Icon name="x" size={14} />
        </button>
      </div>
      {children}
    </dialog>
  );
}

export function ConfirmDialog({ open, onClose, onConfirm, title, text, confirmLabel, busy, safe }: {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
  title: string;
  text: ReactNode;
  confirmLabel: string;
  busy?: boolean;
  /** Not destructive: the confirm button is the primary one, not the red one. */
  safe?: boolean;
}) {
  return (
    <Dialog open={open} onClose={onClose} title={title} width={400}>
      <div className="p-5 text-dim">{text}</div>
      <div className="flex justify-end gap-2 px-5 pb-5">
        <button type="button" className="btn" onClick={onClose} data-autofocus>Cancel</button>
        <button type="button" className={`btn ${safe ? "btn-primary" : "btn-danger"}`} onClick={onConfirm} disabled={busy}>
          {confirmLabel}
        </button>
      </div>
    </Dialog>
  );
}
