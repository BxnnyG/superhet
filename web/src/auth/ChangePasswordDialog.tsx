import { useState, type SyntheticEvent } from "react";
import { Alert, Button, Dialog, PasswordField } from "../ui";
import { AuthError, type AuthClient } from "./api";
import { passwordLengthError } from "./password";
import { notifyUnauthorized } from "./session";

interface Props {
  client: AuthClient;
  onClose: () => void;
}

export function ChangePasswordDialog({ client, onClose }: Props) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [currentError, setCurrentError] = useState<string | null>(null);

  const errors = submitted
    ? {
        current: current === "" ? "Enter your current password." : null,
        next: passwordLengthError(next),
        confirm: confirm !== next ? "The passwords do not match." : null,
      }
    : { current: null, next: null, confirm: null };

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setSubmitted(true);
    setError(null);
    setCurrentError(null);
    if (current === "" || passwordLengthError(next) || confirm !== next) return;
    setBusy(true);
    try {
      await client.changePassword({ currentPassword: current, newPassword: next });
      setDone(true);
    } catch (err) {
      if (err instanceof AuthError && err.kind === "unauthenticated") {
        notifyUnauthorized();
        return;
      }
      if (err instanceof AuthError && err.kind === "invalid_credentials") {
        setCurrentError("The current password is not correct.");
      } else {
        setError(err instanceof AuthError ? err.message : "Could not change the password.");
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title="Change password" onClose={onClose}>
      {done ? (
        <>
          <Alert tone="info">Your password was changed.</Alert>
          <Button variant="primary" onClick={onClose}>
            Done
          </Button>
        </>
      ) : (
        <form onSubmit={submit} noValidate className="flex flex-col gap-4">
          {error && <Alert>{error}</Alert>}
          <PasswordField
            label="Current password"
            name="current-password"
            autoComplete="current-password"
            value={current}
            error={errors.current ?? currentError}
            onChange={(e) => setCurrent(e.target.value)}
          />
          <PasswordField
            label="New password"
            name="new-password"
            autoComplete="new-password"
            value={next}
            error={errors.next}
            hint="At least 12 characters."
            onChange={(e) => setNext(e.target.value)}
          />
          <PasswordField
            label="Confirm new password"
            name="confirm-password"
            autoComplete="new-password"
            value={confirm}
            error={errors.confirm}
            onChange={(e) => setConfirm(e.target.value)}
          />
          <div className="flex justify-end gap-2">
            <Button onClick={onClose}>Cancel</Button>
            <Button type="submit" variant="primary" disabled={busy}>
              {busy ? "Saving…" : "Change password"}
            </Button>
          </div>
        </form>
      )}
    </Dialog>
  );
}
