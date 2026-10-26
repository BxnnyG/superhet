// Fixtures for tests of the auth gate and the role-aware UI (no test framework imported here).

import type { AuthClient, AuthStatus, GuestLimits, Role } from "./api";
import type { AuthContextValue } from "./context";
import { permissionsFor } from "./permissions";

export const SIGNED_OUT: AuthStatus = {
  setupRequired: false,
  authenticated: false,
  username: null,
  userId: null,
  role: null,
  kind: null,
  mustChangePassword: false,
  guest: { enabled: false, privacyText: null },
};

export function makeStatus(over: Partial<AuthStatus> = {}): AuthStatus {
  return { ...SIGNED_OUT, ...over };
}

/** A signed-in account of `role`. */
export function accountStatus(role: Role, username = "alice", userId = 7): AuthStatus {
  return makeStatus({ authenticated: true, username, userId, role, kind: "user" });
}

export function guestStatus(): AuthStatus {
  return makeStatus({
    authenticated: true,
    role: "listener",
    kind: "guest",
    guest: { enabled: true, privacyText: "We log your IP." },
  });
}

export const GUEST_LIMITS: GuestLimits = {
  idleTimeoutSecs: 600,
  maxVfos: 1,
  allowedModes: ["wfm", "nfm", "am"],
  freqMinHz: null,
  freqMaxHz: null,
  showDecoderEvents: false,
};

const reject = () => Promise.reject(new Error("not stubbed"));

export function stubClient(over: Partial<AuthClient> = {}): AuthClient {
  return {
    status: () => Promise.resolve(SIGNED_OUT),
    setup: () => Promise.resolve(),
    login: () => Promise.resolve(),
    logout: () => Promise.resolve(),
    changePassword: () => Promise.resolve(),
    guestStart: () => Promise.resolve(GUEST_LIMITS),
    previewInvite: reject,
    acceptInvite: () => Promise.resolve(),
    ...over,
  };
}

/** Auth context value for rendering components inside a session. */
export function authValue(
  status: AuthStatus,
  over: Partial<AuthContextValue> & { limits?: GuestLimits | null } = {},
): AuthContextValue {
  const { limits = null, ...rest } = over;
  const permissions = permissionsFor(status, limits);
  return {
    username: status.kind === "guest" ? "Guest" : (status.username ?? ""),
    userId: status.userId,
    role: permissions.role,
    kind: permissions.kind,
    permissions,
    client: stubClient(),
    signOut: () => Promise.resolve(),
    ...rest,
  };
}
