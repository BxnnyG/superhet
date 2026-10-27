// What the signed-in caller may do, derived from the role (docs/api-users.md, "Roles"). The
// server enforces all of it; the UI uses this to hide or disable what would be answered with 403.

import type { AuthStatus, GuestLimits, Role } from "./api";

export interface Permissions {
  role: Role;
  kind: "user" | "guest";
  /** Operators and admins: centre, rate, gain, lock, profiles, bookmark edits. */
  isOperator: boolean;
  isAdmin: boolean;
  isGuest: boolean;
  /** `POST /tune` (centre, sample rate, gain, ppm, bias tee). */
  canTune: boolean;
  /** Take and release the tuning lock. */
  canLock: boolean;
  /** `DELETE .../lock?force=true`: release other accounts' locks. */
  canForceUnlock: boolean;
  /** List, save, load and delete profiles. */
  canProfiles: boolean;
  canReadBookmarks: boolean;
  canEditBookmarks: boolean;
  /** Clear the decoder history. */
  canClearHistory: boolean;
  /** List plugins and start / stop decoder instances (operators and admins). */
  canUsePlugins: boolean;
  /** Install and remove plugins (`/admin` console). */
  canInstallPlugins: boolean;
  /** Show the decoder tab (guests only while the policy shows decoder events). */
  canSeeDecoders: boolean;
  /** Operators and admins edit and hear every VFO. */
  seesAllVfos: boolean;
  /** Own sessions page (/me/sessions): accounts only. */
  canManageSessions: boolean;
  /** `/admin` console. */
  canAdmin: boolean;
  /** Identity of the lock holder record: `u:<id>`; null = cannot hold a lock; undefined = unknown. */
  holderId: string | null | undefined;
  /** Policy of a guest session. */
  guestLimits: GuestLimits | null;
}

function build(
  role: Role,
  kind: "user" | "guest",
  userId: number | null | undefined,
  guestLimits: GuestLimits | null,
): Permissions {
  const guest = kind === "guest";
  const operator = !guest && (role === "operator" || role === "admin");
  const admin = !guest && role === "admin";
  let holderId: string | null | undefined;
  if (guest) holderId = null;
  else if (userId === undefined) holderId = undefined;
  else holderId = userId === null ? null : `u:${userId}`;
  return {
    role,
    kind,
    isOperator: operator,
    isAdmin: admin,
    isGuest: guest,
    canTune: operator,
    canLock: operator,
    canForceUnlock: admin,
    canProfiles: operator,
    canReadBookmarks: !guest,
    canEditBookmarks: operator,
    canClearHistory: operator,
    canUsePlugins: operator,
    canInstallPlugins: admin,
    canSeeDecoders: !guest || guestLimits?.showDecoderEvents === true,
    seesAllVfos: operator,
    canManageSessions: !guest,
    canAdmin: admin,
    holderId,
    guestLimits,
  };
}

/** Without sign-in (tests, old single-user setups) everything is allowed. */
export const FULL_ACCESS: Permissions = build("admin", "user", undefined, null);

export function permissionsFor(status: AuthStatus, guestLimits: GuestLimits | null): Permissions {
  const guest = status.kind === "guest";
  return build(
    status.role ?? "listener",
    guest ? "guest" : "user",
    guest ? null : status.userId,
    guest ? guestLimits : null,
  );
}

export const ROLE_LABEL: Record<Role, string> = {
  admin: "Admin",
  operator: "Operator",
  listener: "Listener",
};

/** Label of the signed-in caller for badges: "Admin", "Guest", ... */
export function roleLabel(p: Pick<Permissions, "role" | "isGuest">): string {
  return p.isGuest ? "Guest" : ROLE_LABEL[p.role];
}

// Tooltips for disabled controls.
export const NEEDS_OPERATOR = "Operators and admins only";
export const NEEDS_ACCOUNT = "Sign in with an account to use this";
export const NOT_YOUR_VFO = "This receiver belongs to someone else";

interface VfoOwnership {
  mine?: boolean;
  shared?: boolean;
}

/** Own VFOs are editable for everybody; operators and admins may change any VFO. */
export function canModifyVfo(p: Permissions, vfo: VfoOwnership): boolean {
  return p.seesAllVfos || vfo.mine !== false;
}

/** Own and shared VFOs can be heard by everybody; operators and admins hear all. */
export function canListenVfo(p: Permissions, vfo: VfoOwnership): boolean {
  return p.seesAllVfos || vfo.mine !== false || vfo.shared === true;
}

/** A guest policy forbids this mode (an empty list means no restriction was reported). */
export function modeAllowed(p: Permissions, mode: string): boolean {
  const limits = p.guestLimits;
  return !limits || limits.allowedModes.length === 0 || limits.allowedModes.includes(mode);
}

export function frequencyAllowed(p: Permissions, hz: number): boolean {
  const l = p.guestLimits;
  if (!l) return true;
  return (l.freqMinHz === null || hz >= l.freqMinHz) && (l.freqMaxHz === null || hz <= l.freqMaxHz);
}

/** The per-client VFO cap that is known up front (guests only); null = unknown or unlimited. */
export function knownVfoLimit(p: Permissions): number | null {
  return p.guestLimits && p.guestLimits.maxVfos > 0 ? p.guestLimits.maxVfos : null;
}

export function ownVfoCount(vfos: readonly VfoOwnership[]): number {
  return vfos.filter((v) => v.mine === true).length;
}
