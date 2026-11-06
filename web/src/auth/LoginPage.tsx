import { useEffect, useRef, useState, type SyntheticEvent } from "react";
import { Alert, Button, KeyIcon, PasswordField, TextField, cn } from "../ui";
import { BUTTON_SIZE, BUTTON_VARIANT } from "../ui/Button";
import { AuthError, oidcErrorMessage, type AuthClient, type LoginOptions } from "./api";
import { AuthLayout } from "./AuthLayout";
import { formatCountdown, useCooldown } from "./useCooldown";

interface Props {
  client: AuthClient;
  onSuccess: (username: string) => void;
  /** Shown above the form, e.g. when the session expired. */
  notice?: string;
  /** Guest access is on: offer "Continue as guest". */
  onGuest?: () => void;
  /** Single sign-on button and whether the password form is shown (default: password only). */
  options?: LoginOptions;
}

const PASSWORD_ONLY: LoginOptions = { sso: null, localLogin: true };

/** The `oidc_error` the server appended to the URL after a failed SSO login, then removed. */
function takeOidcError(): string | null {
  const params = new URLSearchParams(window.location.search);
  const code = params.get("oidc_error");
  if (code === null) return null;
  params.delete("oidc_error");
  const rest = params.toString();
  window.history.replaceState(
    window.history.state,
    "",
    `${window.location.pathname}${rest ? `?${rest}` : ""}${window.location.hash}`,
  );
  return oidcErrorMessage(code);
}

export function LoginPage({ client, onSuccess, notice, onGuest, options = PASSWORD_ONLY }: Props) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { remaining, start } = useCooldown();
  const passwordRef = useRef<HTMLInputElement>(null);
  const [ssoError, setSsoError] = useState<string | null>(null);
  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- one-time read of the redirect
    setSsoError(takeOidcError());
  }, []);

  const { sso, localLogin } = options;
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

  const separator = (
    <div
      role="separator"
      className="flex items-center gap-3 text-xs text-fg-subtle before:h-px before:flex-1 before:bg-border after:h-px after:flex-1 after:bg-border"
    >
      or
    </div>
  );

  return (
    <AuthLayout title="Sign in" subtitle="Sign in to your Superhet receiver.">
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        {notice && !error && !ssoError && <Alert tone="info">{notice}</Alert>}
        {error && <Alert>{error}</Alert>}
        {ssoError && <Alert>{ssoError}</Alert>}
        {sso && (
          <a
            href={sso.loginUrl}
            className={cn(
              "focus-ring inline-flex shrink-0 items-center justify-center rounded-md font-medium",
              "transition-colors duration-150",
              localLogin ? BUTTON_VARIANT.secondary : BUTTON_VARIANT.primary,
              BUTTON_SIZE.lg,
            )}
          >
            <KeyIcon size={16} />
            Sign in with {sso.name}
          </a>
        )}
        {sso && localLogin && separator}
        {localLogin && (
          <>
            <TextField
              label="Username"
              name="username"
              autoComplete="username"
              autoCapitalize="off"
              spellCheck={false}
              autoFocus={!sso}
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
          </>
        )}
        {onGuest && (
          <>
            {(localLogin || sso) && separator}
            <Button size="lg" onClick={onGuest} disabled={busy}>
              Continue as guest
            </Button>
          </>
        )}
      </form>
    </AuthLayout>
  );
}
