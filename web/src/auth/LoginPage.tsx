import { useRef, useState, type SyntheticEvent } from "react";
import { Alert, Button, PasswordField, TextField } from "../ui";
import { AuthError, type AuthClient } from "./api";
import { AuthLayout } from "./AuthLayout";
import { formatCountdown, useCooldown } from "./useCooldown";

interface Props {
  client: AuthClient;
  onSuccess: (username: string) => void;
  /** Shown above the form, e.g. when the session expired. */
  notice?: string;
  /** Guest access is on: offer "Continue as guest". */
  onGuest?: () => void;
}

export function LoginPage({ client, onSuccess, notice, onGuest }: Props) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { remaining, start } = useCooldown();
  const passwordRef = useRef<HTMLInputElement>(null);

  const limited = remaining > 0;

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy || limited) return;
    if (username.trim() === "" || password === "") {
      setError("Enter your username and password.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await client.login({ username: username.trim(), password });
      onSuccess(username.trim());
    } catch (err) {
      if (err instanceof AuthError && err.kind === "rate_limited") {
        start(err.retryAfter ?? 30);
        setError("Too many sign-in attempts. Please wait before trying again.");
      } else if (err instanceof AuthError && err.kind === "invalid_credentials") {
        setError("Invalid username or password.");
        setPassword("");
        passwordRef.current?.focus();
      } else {
        setError(err instanceof AuthError ? err.message : "Sign-in failed.");
      }
      setBusy(false);
    }
  };

  return (
    <AuthLayout title="Sign in" subtitle="Sign in to your Superhet receiver.">
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        {notice && !error && <Alert tone="info">{notice}</Alert>}
        {error && <Alert>{error}</Alert>}
        <TextField
          label="Username"
          name="username"
          autoComplete="username"
          autoCapitalize="off"
          spellCheck={false}
          autoFocus
          value={username}
          onChange={(e) => setUsername(e.target.value)}
        />
        <PasswordField
          label="Password"
          name="password"
          autoComplete="current-password"
          ref={passwordRef}
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <Button type="submit" variant="primary" size="lg" disabled={busy || limited}>
          {busy
            ? "Signing in…"
            : limited
              ? `Try again in ${formatCountdown(remaining)}`
              : "Sign in"}
        </Button>
        {onGuest && (
          <>
            <div
              role="separator"
              className="flex items-center gap-3 text-xs text-fg-subtle before:h-px before:flex-1 before:bg-border after:h-px after:flex-1 after:bg-border"
            >
              or
            </div>
            <Button size="lg" onClick={onGuest} disabled={busy}>
              Continue as guest
            </Button>
          </>
        )}
      </form>
    </AuthLayout>
  );
}
