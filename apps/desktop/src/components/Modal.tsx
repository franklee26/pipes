import { useEffect, useId, useRef, type ReactNode } from "react";

interface ModalProps {
  title: string;
  busy?: boolean;
  onClose(): void;
  children: ReactNode;
  wide?: boolean;
  onReturnFocus?(): void;
}

export function Modal({ title, busy = false, onClose, children, wide, onReturnFocus }: ModalProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();

  useEffect(() => {
    const dialog = ref.current;
    const previousFocus = document.activeElement;
    dialog?.showModal();
    dialog?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    return () => {
      dialog?.close();
      if (previousFocus instanceof HTMLElement && previousFocus.isConnected && !previousFocus.matches(":disabled")) {
        previousFocus.focus();
      } else {
        onReturnFocus?.();
      }
    };
  }, []);

  return (
    <dialog
      ref={ref}
      className={wide ? "modal modal-wide" : "modal"}
      aria-labelledby={titleId}
      aria-busy={busy}
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <header className="modal-heading">
        <h2 id={titleId}>{title}</h2>
        <button
          type="button"
          className="close-button"
          aria-label="Close dialog"
          onClick={onClose}
          disabled={busy}
        >
          &times;
        </button>
      </header>
      <div className="modal-body">{children}</div>
    </dialog>
  );
}
