import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { Alert, Button } from "../ui";
import { AuthLayout } from "./AuthLayout";
import { createAuthClient, type AuthClient } from "./api";
import { AuthContext } from "./context";
import { LoginPage } from "./LoginPage";
import { notifyUnauthorized, onSessionLost, resetSession } from "./session";
import { SetupPage } from "./SetupPage";

type State =
  | { kind: "loading" }
  | { kind: "unreachable" }
  | { kind: "setup" }
  | { kind: "login"; notice?: string }
  | { kind: "app"; username: string };

const defaultClient = createAuthClient();

interface Props {
  children: ReactNode;
  client?: AuthClient;
}

/** Checks the session on load and renders setup, login or the app. */
export function AuthGate({ children, client = defaultClient }: Props) {
  const [state, setState] = useState<State>({ kind: "loading" });

  const refresh = useCallback(
    async (fallbackUser?: string) => {
      try {
        const st = await client.status();
        if (st.setupRequired) setState({ kind: "setup" });
        else if (st.authenticated) {
          resetSession();
          setState({ kind: "app", username: st.username ?? fallbackUser ?? "" });
        } else setState({ kind: "login" });
      } catch {
        if (fallbackUser) {
          resetSession();
          setState({ kind: "app", username: fallbackUser });
        } else setState({ kind: "unreachable" });
      }
    },
    [client],
  );

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- initial session check
    void refresh();
  }, [refresh]);

  useEffect(
    () =>
      onSessionLost(() =>
        setState((s) =>
          s.kind === "app"
            ? { kind: "login", notice: "Your session has ended. Please sign in again." }
            : s,
        ),
      ),
    [],
  );

  const signOut = useCallback(async () => {
    try {
      await client.logout();
    } catch {
      // The local session ends regardless; the cookie expires server-side.
    }
    notifyUnauthorized();
    setState({ kind: "login" });
  }, [client]);

  const ctx = useMemo(
    () => (state.kind === "app" ? { username: state.username, client, signOut } : null),
    [state, client, signOut],
  );

  switch (state.kind) {
    case "loading":
      return (
        <AuthLayout title="Superhet">
          <p role="status" className="text-sm text-fg-muted">
            Checking session…
          </p>
        </AuthLayout>
      );
    case "unreachable":
      return (
        <AuthLayout title="Cannot reach the server">
          <Alert>The Superhet API did not respond. Check that it is running.</Alert>
          <Button variant="primary" onClick={() => void refresh()}>
            Retry
          </Button>
        </AuthLayout>
      );
    case "setup":
      return (
        <SetupPage
          client={client}
          onSuccess={(u) => void refresh(u)}
          onAlreadySetup={() => setState({ kind: "login" })}
        />
      );
    case "login":
      return <LoginPage client={client} notice={state.notice} onSuccess={(u) => void refresh(u)} />;
    case "app":
      return <AuthContext value={ctx}>{children}</AuthContext>;
  }
}
