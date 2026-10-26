import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { setLockIdentity } from "../state/locks";
import { Alert, Button } from "../ui";
import { inviteTokenOf, navigate, usePath } from "../router";
import { AuthLayout } from "./AuthLayout";
import {
  createAuthClient,
  parseStatus,
  type AuthClient,
  type AuthStatus,
  type GuestInfo,
  type GuestLimits,
} from "./api";
import { AuthContext, type AuthContextValue } from "./context";
import { ForcedPasswordPage } from "./ForcedPasswordPage";
import { GuestPage } from "./GuestPage";
import { InvitePage } from "./InvitePage";
import { LoginPage } from "./LoginPage";
import { permissionsFor } from "./permissions";
import {
  notifyUnauthorized,
  onRecheckRequested,
  onSessionLost,
  onSessionStatus,
  resetSession,
} from "./session";
import { SetupPage } from "./SetupPage";

type State =
  | { kind: "loading" }
  | { kind: "unreachable" }
  | { kind: "setup" }
  | { kind: "login"; notice?: string; guest: GuestInfo }
  | { kind: "guest"; guest: GuestInfo }
  | { kind: "password"; username: string | null }
  | { kind: "invite"; token: string; signedInAs: string | null }
  | { kind: "app"; status: AuthStatus; limits: GuestLimits | null };

const defaultClient = createAuthClient();
const NO_GUEST: GuestInfo = { enabled: false, privacyText: null };

interface Props {
  children: ReactNode;
  client?: AuthClient;
}

/**
 * Checks the session on load and renders setup, login (with guest access when the operator
 * enabled it), the forced password change, an invitation page or the app. A kick, a disabled
 * account or a role change surfaces here: the sockets re-check the status when they close.
 */
export function AuthGate({ children, client = defaultClient }: Props) {
  const [state, setState] = useState<State>({ kind: "loading" });
  const stateRef = useRef(state);
  useEffect(() => {
    stateRef.current = state;
  });
  const path = usePath();
  const inviteToken = inviteTokenOf(path);

  const apply = useCallback(
    async (st: AuthStatus) => {
      if (st.setupRequired) {
        setState({ kind: "setup" });
      } else if (inviteTokenOf(window.location.pathname) !== null) {
        setState({
          kind: "invite",
          token: inviteTokenOf(window.location.pathname) ?? "",
          signedInAs: st.authenticated ? st.username : null,
        });
      } else if (st.authenticated && st.mustChangePassword) {
        setState({ kind: "password", username: st.username });
      } else if (st.authenticated) {
        // A guest session's limits come with the (idempotent) start request.
        let limits: GuestLimits | null = null;
        if (st.kind === "guest") {
          try {
            limits = await client.guestStart();
          } catch {
            limits = null;
          }
        }
        resetSession();
        setState({ kind: "app", status: st, limits });
      } else {
        setState({ kind: "login", guest: st.guest });
      }
    },
    [client],
  );

  const refresh = useCallback(
    async (fallbackUser?: string) => {
      try {
        await apply(await client.status());
      } catch {
        if (fallbackUser) {
          resetSession();
          setState({
            kind: "app",
            status: {
              setupRequired: false,
              authenticated: true,
              username: fallbackUser,
              userId: null,
              role: "admin",
              kind: "user",
              mustChangePassword: false,
              guest: NO_GUEST,
            },
            limits: null,
          });
        } else setState({ kind: "unreachable" });
      }
    },
    [client, apply],
  );

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect -- initial session check
    void refresh();
  }, [refresh]);

  // The session ended (kick, expiry, disabled account): back to login with a notice.
  useEffect(
    () =>
      onSessionLost(() =>
        setState((s) => {
          if (s.kind !== "app") return s;
          const guest = s.status.kind === "guest";
          return {
            kind: "login",
            guest: s.status.guest,
            notice: guest
              ? "Your guest session has ended. You can start a new one."
              : "Your session has ended. Please sign in again.",
          };
        }),
      ),
    [],
  );

  // A socket closed and the server still knows us: pick up a changed role or a forced password
  // change. A data route answering `password_change_required` asks for a re-read as well.
  useEffect(
    () =>
      onSessionStatus((raw) => {
        try {
          const st = parseStatus(raw);
          const s = stateRef.current;
          if (s.kind !== "app") return;
          const a = s.status;
          const changed =
            st.role !== a.role ||
            st.kind !== a.kind ||
            st.userId !== a.userId ||
            st.mustChangePassword;
          if (changed) void apply(st);
        } catch {
          // Unreadable status: keep the session as it is.
        }
      }),
    [apply],
  );
  useEffect(() => onRecheckRequested(() => void refresh()), [refresh]);

  const signOut = useCallback(async () => {
    try {
      await client.logout();
    } catch {
      // The local session ends regardless; the cookie expires server-side.
    }
    notifyUnauthorized();
    if (window.location.pathname !== "/") navigate("/", { replace: true });
    // Keep the guest offer of the session that just ended on the login page.
    setState((s) => ({ kind: "login", guest: s.kind === "app" ? s.status.guest : NO_GUEST }));
  }, [client]);

  const ctx = useMemo<AuthContextValue | null>(() => {
    if (state.kind !== "app") return null;
    const st = state.status;
    const permissions = permissionsFor(st, state.limits);
    return {
      username: st.kind === "guest" ? "Guest" : (st.username ?? ""),
      userId: st.userId,
      role: permissions.role,
      kind: permissions.kind,
      permissions,
      client,
      signOut,
    };
  }, [state, client, signOut]);
  const holder = ctx?.permissions.holderId;
  useEffect(() => {
    setLockIdentity(ctx === null ? undefined : holder);
  }, [ctx, holder]);

  // An invitation link wins over everything but the setup wizard; leaving the page returns home.
  const leaveInvite = () => {
    navigate("/", { replace: true });
    void refresh();
  };

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
          onAlreadySetup={() => setState({ kind: "login", guest: NO_GUEST })}
        />
      );
    case "login":
      return (
        <LoginPage
          client={client}
          notice={state.notice}
          onSuccess={(u) => void refresh(u)}
          onGuest={
            state.guest.enabled ? () => setState({ kind: "guest", guest: state.guest }) : undefined
          }
        />
      );
    case "guest":
      return (
        <GuestPage
          client={client}
          guest={state.guest}
          onStarted={() => void refresh()}
          onBack={() => setState({ kind: "login", guest: state.guest })}
        />
      );
    case "password":
      return (
        <ForcedPasswordPage
          client={client}
          username={state.username}
          onChanged={() => void refresh()}
          onSignOut={() => void signOut()}
        />
      );
    case "invite":
      return (
        <InvitePage
          client={client}
          token={inviteToken ?? state.token}
          signedInAs={state.signedInAs}
          onAccepted={() => {
            // The token must not stay in the address bar or the history.
            navigate("/", { replace: true });
            void refresh();
          }}
          onCancel={leaveInvite}
        />
      );
    case "app":
      return <AuthContext value={ctx}>{children}</AuthContext>;
  }
}
