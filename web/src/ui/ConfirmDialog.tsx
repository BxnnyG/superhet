import type { ReactNode } from "react";
import { Button, type ButtonVariant } from "./Button";
import { Dialog } from "./Dialog";

interface Props {
  title: string;
  children: ReactNode;
  confirmLabel: string;
  confirmVariant?: ButtonVariant;
  cancelLabel?: string;
  /** Disables both buttons while the confirmed action runs. */
  busy?: boolean;
  /** Omit to show an information-only dialog with just the close button. */
  onConfirm?: () => void;
  onCancel: () => void;
}

/** Small confirmation dialog (delete, overwrite, ...). Esc and backdrop cancel. */
export function ConfirmDialog({
  title,
  children,
  confirmLabel,
  confirmVariant = "primary",
  cancelLabel = "Cancel",
  busy,
  onConfirm,
  onCancel,
}: Props) {
  return (
    <Dialog title={title} onClose={onCancel}>
      <div className="text-sm text-fg-muted">{children}</div>
      <div className="flex justify-end gap-2">
        <Button variant="secondary" onClick={onCancel} disabled={busy}>
          {onConfirm ? cancelLabel : "OK"}
        </Button>
        {onConfirm && (
          <Button variant={confirmVariant} onClick={onConfirm} disabled={busy}>
            {confirmLabel}
          </Button>
        )}
      </div>
    </Dialog>
  );
}
