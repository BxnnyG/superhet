import { useEffect, useState, type SyntheticEvent } from "react";
import { Alert, Badge, Button, PasswordField, TextField } from "../ui";
import { AuthError, PASSWORD_MIN, USERNAME_RE, type AuthClient, type InvitePreview } from "./api";
import { AuthLayout } from "./AuthLayout";
import { passwordLengthError } from "./password";
import { ROLE_LABEL } from "./permissions";
import { StrengthBar } from "./StrengthBar";

interface Props {
  client: AuthClient;
  token: string;
  /** Somebody is signed in on this browser; accepting switches to the new account. */
  signedInAs?: string | null;
  /** The account exists and its session cookie is set. */
  onAccepted: () => void;
  onCancel: () => void;
}

type Load = { kind: "loading" } | { kind: "ok"; invite: InvitePreview } | { kind: "bad" };

export function validateInvite(v: { username: string; password: string; confirm: string }) {
  const e: { username?: string; password?: string; confirm?: string } = {};
  if (!USERNAME_RE.test(v.username)) {
    e.username = "1 to 64 characters: letters, digits, dot, underscore or hyphen.";
  }
  const pw = passwordLengthError(v.password);
  if (pw) e.password = pw;
  if (v.confirm !== v.password) e.confirm = "The passwords do not match.";
  return e;
}

/** `/invite/<token>`: pick a user name and password for the role the invitation fixes. */
export function InvitePage({ client, token, signedInAs, onAccepted, onCancel }: Props) {
  const [load, setLoad] = useState<Load>({ kind: "loading" });
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [nameError, setNameError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client
      .previewInvite(token)
      .then((invite) => live && setLoad({ kind: "ok", invite }))
      .catch(() => live && setLoad({ kind: "bad" }));
    return () => {
      live = false;
    };
  }, [client, token]);

  if (load.kind === "loading") {
    return (
      <AuthLayout title="Invitation">
        <p role="status" className="text-sm text-fg-muted">
          Checking the invitation…
        </p>
      </AuthLayout>
    );
  }
  if (load.kind === "bad") {
    return (
      <AuthLayout title="Invitation not valid">
        <Alert tone="warn">
          This invitation link is unknown, already used, revoked or expired. Ask the administrator
          for a new one.
        </Alert>
        <Button variant="primary" size="lg" onClick={onCancel}>
          Go to sign in
        </Button>
      </AuthLayout>
    );
  }

  const errors = submitted ? validateInvite({ username, password, confirm }) : {};

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setSubmitted(true);
    setError(null);
    setNameError(null);
    if (Object.keys(validateInvite({ username, password, confirm })).length > 0) return;
    setBusy(true);
    try {
      await client.acceptInvite({ token, username, password });
      onAccepted();
    } catch (err) {
      if (err instanceof AuthError && err.kind === "username_taken") {
        setNameError("This username is already taken. Pick another one.");
      } else if (err instanceof AuthError && err.kind === "invalid_invite") {
        setLoad({ kind: "bad" });
      } else if (err instanceof AuthError && err.kind === "rate_limited") {
        setError("Too many attempts. Please wait a moment and try again.");
      } else {
        setError(err instanceof AuthError ? err.message : "Could not accept the invitation.");
      }
      setBusy(false);
    }
  };

  const { invite } = load;
  return (
    <AuthLayout
      title="Join Superhet"
      subtitle={
        invite.invitedBy
          ? `${invite.invitedBy} invited you. Choose your sign-in details.`
          : "You were invited. Choose your sign-in details."
      }
    >
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        <div className="flex items-center gap-2 text-sm text-fg-muted">
          Your role
          <Badge tone="accent">{ROLE_LABEL[invite.role]}</Badge>
        </div>
        {signedInAs && (
          <Alert tone="info">
            You are signed in as {signedInAs} in this browser. Accepting switches to the new
            account.
          </Alert>
        )}
        {error && <Alert>{error}</Alert>}
        <TextField
          label="Username"
          name="username"
          autoComplete="username"
          autoCapitalize="off"
          spellCheck={false}
          autoFocus
          maxLength={64}
          value={username}
          error={errors.username ?? nameError}
          onChange={(e) => setUsername(e.target.value)}
        />
        <div className="flex flex-col gap-2">
          <PasswordField
            label="Password"
            name="new-password"
            autoComplete="new-password"
            minLength={PASSWORD_MIN}
            value={password}
            error={errors.password}
            hint={`At least ${PASSWORD_MIN} characters. A long passphrase works well.`}
            onChange={(e) => setPassword(e.target.value)}
          />
          <StrengthBar password={password} />
        </div>
        <PasswordField
          label="Confirm password"
          name="confirm-password"
          autoComplete="new-password"
          value={confirm}
          error={errors.confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        <Button type="submit" variant="primary" size="lg" disabled={busy}>
          {busy ? "Creating account…" : "Create account"}
        </Button>
        <Button variant="ghost" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </form>
    </AuthLayout>
  );
}
