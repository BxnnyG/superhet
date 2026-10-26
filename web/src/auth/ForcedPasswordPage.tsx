import { useState, type SyntheticEvent } from "react";
import { Alert, Button, PasswordField } from "../ui";
import { AuthError, type AuthClient } from "./api";
import { AuthLayout } from "./AuthLayout";
import { passwordLengthError } from "./password";
import { StrengthBar } from "./StrengthBar";

interface Props {
  client: AuthClient;
  username: string | null;
  /** The password was replaced; the gate re-reads the status. */
  onChanged: () => void;
  onSignOut: () => void;
}

/**
 * Shown while the account still has a one-time password (a new account created by an admin, or a
 * password reset): every other route answers 403 `password_change_required` until it is replaced.
 */
export function ForcedPasswordPage({ client, username, onChanged, onSignOut }: Props) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [currentError, setCurrentError] = useState<string | null>(null);

  const errors = submitted
    ? {
        current: current === "" ? "Enter the one-time password you were given." : null,
        next:
          passwordLengthError(next) ?? (next === current ? "Choose a different password." : null),
        confirm: confirm !== next ? "The passwords do not match." : null,
      }
    : { current: null, next: null, confirm: null };

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setSubmitted(true);
    setError(null);
    setCurrentError(null);
    if (current === "" || passwordLengthError(next) || next === current || confirm !== next) return;
    setBusy(true);
    try {
      await client.changePassword({ currentPassword: current, newPassword: next });
      onChanged();
    } catch (err) {
      if (err instanceof AuthError && err.kind === "invalid_credentials") {
        setCurrentError("The one-time password is not correct.");
      } else if (err instanceof AuthError && err.kind === "unauthenticated") {
        onChanged();
        return;
      } else {
        setError(err instanceof AuthError ? err.message : "Could not change the password.");
      }
      setBusy(false);
    }
  };

  return (
    <AuthLayout
      title="Choose a new password"
      subtitle={
        username
          ? `${username}, your password was set by an administrator. Replace it to continue.`
          : "Your password was set by an administrator. Replace it to continue."
      }
    >
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        {error && <Alert>{error}</Alert>}
        <PasswordField
          label="One-time password"
          name="current-password"
          autoComplete="current-password"
          autoFocus
          value={current}
          error={errors.current ?? currentError}
          onChange={(e) => setCurrent(e.target.value)}
        />
        <div className="flex flex-col gap-2">
          <PasswordField
            label="New password"
            name="new-password"
            autoComplete="new-password"
            value={next}
            error={errors.next}
            hint="At least 12 characters. A long passphrase works well."
            onChange={(e) => setNext(e.target.value)}
          />
          <StrengthBar password={next} />
        </div>
        <PasswordField
          label="Confirm new password"
          name="confirm-password"
          autoComplete="new-password"
          value={confirm}
          error={errors.confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        <Button type="submit" variant="primary" size="lg" disabled={busy}>
          {busy ? "Saving…" : "Change password"}
        </Button>
        <Button variant="ghost" onClick={onSignOut} disabled={busy}>
          Sign out
        </Button>
      </form>
    </AuthLayout>
  );
}
