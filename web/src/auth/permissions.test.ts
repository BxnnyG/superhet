import { describe, expect, it } from "vitest";
import {
  FULL_ACCESS,
  canListenVfo,
  canModifyVfo,
  frequencyAllowed,
  knownVfoLimit,
  modeAllowed,
  ownVfoCount,
  permissionsFor,
  roleLabel,
} from "./permissions";
import { GUEST_LIMITS, accountStatus, guestStatus } from "./testing";

const admin = permissionsFor(accountStatus("admin"), null);
const operator = permissionsFor(accountStatus("operator"), null);
const listener = permissionsFor(accountStatus("listener"), null);
const guest = permissionsFor(guestStatus(), GUEST_LIMITS);

describe("role matrix", () => {
  it("admin can do everything", () => {
    expect(admin).toMatchObject({
      canTune: true,
      canLock: true,
      canForceUnlock: true,
      canProfiles: true,
      canEditBookmarks: true,
      canClearHistory: true,
      canUsePlugins: true,
      canInstallPlugins: true,
      canAdmin: true,
      seesAllVfos: true,
      canManageSessions: true,
    });
  });

  it("operator tunes and edits but has no console and cannot force-unlock", () => {
    expect(operator).toMatchObject({
      canTune: true,
      canLock: true,
      canProfiles: true,
      canEditBookmarks: true,
      canClearHistory: true,
      seesAllVfos: true,
    });
    expect(operator.canAdmin).toBe(false);
    expect(operator.canForceUnlock).toBe(false);
    // Operators start and stop decoders; installing is the admin's.
    expect(operator.canUsePlugins).toBe(true);
    expect(operator.canInstallPlugins).toBe(false);
  });

  it("listener only listens: no tune, lock, profiles, bookmark edits or console", () => {
    expect(listener).toMatchObject({
      canTune: false,
      canLock: false,
      canProfiles: false,
      canEditBookmarks: false,
      canClearHistory: false,
      canUsePlugins: false,
      canInstallPlugins: false,
      canAdmin: false,
      seesAllVfos: false,
      canReadBookmarks: true,
      canSeeDecoders: true,
      canManageSessions: true,
    });
  });

  it("guest has no bookmarks, no sessions page and decoders only by policy", () => {
    expect(guest).toMatchObject({
      isGuest: true,
      canReadBookmarks: false,
      canManageSessions: false,
      canSeeDecoders: false,
      canUsePlugins: false,
      canTune: false,
      canAdmin: false,
      holderId: null,
    });
    const withDecoders = permissionsFor(guestStatus(), {
      ...GUEST_LIMITS,
      showDecoderEvents: true,
    });
    expect(withDecoders.canSeeDecoders).toBe(true);
  });

  it("a guest is never an operator even if the status claims another role", () => {
    const sneaky = permissionsFor({ ...guestStatus(), role: "admin" }, GUEST_LIMITS);
    expect(sneaky.canAdmin).toBe(false);
    expect(sneaky.canTune).toBe(false);
  });

  it("without a session everything is allowed (tests, old setups)", () => {
    expect(FULL_ACCESS.canAdmin).toBe(true);
    expect(FULL_ACCESS.holderId).toBeUndefined();
  });

  it("labels the roles", () => {
    expect(roleLabel(admin)).toBe("Admin");
    expect(roleLabel(listener)).toBe("Listener");
    expect(roleLabel(guest)).toBe("Guest");
  });

  it("identifies the lock holder record of an account", () => {
    expect(permissionsFor(accountStatus("operator", "a", 12), null).holderId).toBe("u:12");
  });
});

describe("VFO ownership", () => {
  const own = { mine: true };
  const foreign = { mine: false, shared: false };
  const shared = { mine: false, shared: true };

  it("listeners and guests edit only their own receivers", () => {
    for (const p of [listener, guest]) {
      expect(canModifyVfo(p, own)).toBe(true);
      expect(canModifyVfo(p, foreign)).toBe(false);
      expect(canModifyVfo(p, shared)).toBe(false);
    }
  });

  it("operators and admins edit all", () => {
    for (const p of [operator, admin]) expect(canModifyVfo(p, foreign)).toBe(true);
  });

  it("listening: own and shared, never somebody else's", () => {
    expect(canListenVfo(listener, own)).toBe(true);
    expect(canListenVfo(listener, shared)).toBe(true);
    expect(canListenVfo(listener, foreign)).toBe(false);
    expect(canListenVfo(operator, foreign)).toBe(true);
  });

  it("a daemon without accounts (no flags) leaves everything editable", () => {
    expect(canModifyVfo(listener, {})).toBe(true);
    expect(canListenVfo(listener, {})).toBe(true);
  });

  it("counts own receivers", () => {
    expect(ownVfoCount([own, foreign, shared, own])).toBe(2);
  });
});

describe("guest policy", () => {
  it("knows the per-guest receiver cap up front", () => {
    expect(knownVfoLimit(guest)).toBe(1);
    expect(knownVfoLimit(listener)).toBeNull();
  });

  it("checks modes and the frequency window", () => {
    expect(modeAllowed(guest, "wfm")).toBe(true);
    expect(modeAllowed(guest, "usb")).toBe(false);
    expect(modeAllowed(listener, "usb")).toBe(true);
    const ranged = permissionsFor(guestStatus(), {
      ...GUEST_LIMITS,
      freqMinHz: 88e6,
      freqMaxHz: 108e6,
    });
    expect(frequencyAllowed(ranged, 100e6)).toBe(true);
    expect(frequencyAllowed(ranged, 145e6)).toBe(false);
    expect(frequencyAllowed(guest, 145e6)).toBe(true);
  });
});
