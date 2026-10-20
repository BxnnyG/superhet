import { useState, type SyntheticEvent } from "react";
import { Alert, Button, PasswordField, TextField, cn } from "../ui";
import { AuthError, PASSWORD_MIN, USERNAME_RE, type AuthClient } from "./api";
import { AuthLayout } from "./AuthLayout";
import { passwordLengthError, passwordStrength } from "./password";

interface Props {
  client: AuthClient;
  onSuccess: (username: string) => void;
  /** The server reports that setup was completed elsewhere; go to the login page. */
  onAlreadySetup: () => void;
}

interface Errors {
  username?: string;
  password?: string;
  confirm?: string;
  token?: string;
}

const BAR = ["bg-border-strong", "bg-danger", "bg-warn", "bg-success", "bg-success"] as const;

export function validateSetup(v: {
  username: string;
  password: string;
  confirm: string;
  token: string;
}): Errors {
  const e: Errors = {};
  if (!USERNAME_RE.test(v.username)) {
    e.username = "1 to 64 characters: letters, digits, dot, underscore or hyphen.";
  }
  e.password = passwordLengthError(v.password) ?? undefined;
  if (v.confirm !== v.password) e.confirm = "The passwords do not match.";
  if (v.token.trim() === "") e.token = "Enter the setup token from the server log.";
  for (const k of Object.keys(e) as (keyof Errors)[]) if (!e[k]) delete e[k];
  return e;
}

export function SetupPage({ client, onSuccess, onAlreadySetup }: Props) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [token, setToken] = useState("");
  const [submitted, setSubmitted] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const [tokenError, setTokenError] = useState<string | null>(null);
  const [alreadySetup, setAlreadySetup] = useState(false);

  const errors = submitted ? validateSetup({ username, password, confirm, token }) : {};
  const strength = passwordStrength(password);

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setSubmitted(true);
    setServerError(null);
    setTokenError(null);
    if (Object.keys(validateSetup({ username, password, confirm, token })).length > 0) return;
    setBusy(true);
    try {
      await client.setup({ username, password, setupToken: token.trim() });
      onSuccess(username);
    } catch (err) {
      if (err instanceof AuthError && err.kind === "invalid_setup_token") {
        setTokenError("This setup token is not valid. Copy it again from the server log.");
      } else if (err instanceof AuthError && err.kind === "already_setup") {
        setAlreadySetup(true);
      } else if (err instanceof AuthError && err.kind === "rate_limited") {
        setServerError("Too many attempts. Please wait a moment and try again.");
      } else {
        setServerError(err instanceof AuthError ? err.message : "Setup failed.");
      }
      setBusy(false);
    }
  };

  if (alreadySetup) {
    return (
      <AuthLayout title="Already set up">
        <Alert tone="warn">Setup was already completed. Sign in with the existing account.</Alert>
        <Button variant="primary" size="lg" onClick={onAlreadySetup}>
          Go to sign in
        </Button>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout
      title="Set up Superhet"
      subtitle="Create the administrator account for this receiver."
    >
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        {serverError && <Alert>{serverError}</Alert>}
        <TextField
          label="Username"
          name="username"
          autoComplete="username"
          autoCapitalize="off"
          spellCheck={false}
          autoFocus
          maxLength={64}
          value={username}
          error={errors.username}
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
          {password && (
            <div className="flex items-center gap-2" data-testid="strength">
              <div className="flex flex-1 gap-1" aria-hidden="true">
                {[1, 2, 3, 4].map((i) => (
                  <span
                    key={i}
                    className={cn(
                      "h-1 flex-1 rounded-full",
                      strength.score >= i ? BAR[strength.score] : "bg-border-strong",
                    )}
                  />
                ))}
              </div>
              <span className="w-16 text-right text-xs text-fg-muted">
                Strength: {strength.label}
              </span>
            </div>
          )}
        </div>
        <PasswordField
          label="Confirm password"
          name="confirm-password"
          autoComplete="new-password"
          value={confirm}
          error={errors.confirm}
          onChange={(e) => setConfirm(e.target.value)}
        />
        <TextField
          label="Setup token"
          name="setup-token"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          value={token}
          error={errors.token ?? tokenError}
          hint={
            <>
              Printed once in the API log on first start:{" "}
              <code className="font-mono text-fg-muted">docker compose logs superhet-api</code>
            </>
          }
          onChange={(e) => setToken(e.target.value)}
        />
        <Button type="submit" variant="primary" size="lg" disabled={busy}>
          {busy ? "Creating account…" : "Create account"}
        </Button>
      </form>
    </AuthLayout>
  );
}
