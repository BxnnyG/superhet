import { afterEach, describe, expect, it, vi } from "vitest";
import { createAuthClient, parseStatus } from "./api";
import {
  apiFetch,
  checkSessionAfterClose,
  isSessionLost,
  onRecheckRequested,
  onSessionStatus,
  resetSession,
} from "./session";

const reply = (status: number, body?: unknown, headers: Record<string, string> = {}) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status, headers });

afterEach(() => {
  vi.unstubAllGlobals();
  resetSession();
});

describe("status document", () => {
  it("reads role, kind, forced change and the guest offer", () => {
    expect(
      parseStatus({
        setup_required: false,
        authenticated: true,
        username: "olivia",
        user_id: 2,
        role: "operator",
        kind: "user",
        must_change_password: true,
        guest: { enabled: true, privacy_text: "Notice" },
      }),
    ).toEqual({
      setupRequired: false,
      authenticated: true,
      username: "olivia",
      userId: 2,
      role: "operator",
      kind: "user",
      mustChangePassword: true,
      guest: { enabled: true, privacyText: "Notice" },
    });
  });

  it("a guest has no name and the listener role", () => {
    const s = parseStatus({
      setup_required: false,
      authenticated: true,
      username: null,
      user_id: null,
      role: "listener",
      kind: "guest",
      must_change_password: false,
      guest: { enabled: true, privacy_text: null },
    });
    expect(s).toMatchObject({ kind: "guest", role: "listener", username: null, userId: null });
  });

  it("signed out has no role or kind", () => {
    const s = parseStatus({ setup_required: false, authenticated: false, role: "admin" });
    expect(s).toMatchObject({ role: null, kind: null, guest: { enabled: false } });
  });
});

describe("guest, invite and error mapping", () => {
  it("starts a guest session and reads the limits", async () => {
    const f = vi.fn().mockResolvedValue(
      reply(200, {
        guest: { id: "ab12" },
        limits: {
          idle_timeout_secs: 300,
          max_vfos: 2,
          allowed_modes: ["wfm", "am"],
          freq_min_hz: 88e6,
          freq_max_hz: null,
          show_decoder_events: true,
        },
      }),
    );
    const limits = await createAuthClient(f).guestStart();
    expect(limits).toEqual({
      idleTimeoutSecs: 300,
      maxVfos: 2,
      allowedModes: ["wfm", "am"],
      freqMinHz: 88e6,
      freqMaxHz: null,
      showDecoderEvents: true,
    });
    expect(f.mock.calls[0]?.[0]).toBe("/api/v1/auth/guest");
    expect((f.mock.calls[0]?.[1] as RequestInit).method).toBe("POST");
  });

  it.each([
    [503, { error: "guests_full" }, "guests_full"],
    [429, { error: "too_many_guests" }, "too_many_guests"],
    [404, { error: "not_found" }, "not_found"],
  ])("guest refusal %i maps to %s", async (status, body, kind) => {
    const c = createAuthClient(vi.fn().mockResolvedValue(reply(status, body)));
    await expect(c.guestStart()).rejects.toMatchObject({ kind });
  });

  it("previews an open invite (token is URL-encoded)", async () => {
    const f = vi
      .fn()
      .mockResolvedValue(reply(200, { role: "operator", expires_at_ms: 99, invited_by: "benny" }));
    expect(await createAuthClient(f).previewInvite("a/b")).toEqual({
      role: "operator",
      expiresAtMs: 99,
      invitedBy: "benny",
    });
    expect(f.mock.calls[0]?.[0]).toBe("/api/v1/auth/invites/a%2Fb");
  });

  it("maps invite errors", async () => {
    const gone = createAuthClient(
      vi.fn().mockResolvedValue(reply(404, { error: "invalid_invite" })),
    );
    await expect(gone.previewInvite("x")).rejects.toMatchObject({ kind: "invalid_invite" });
    const taken = createAuthClient(
      vi.fn().mockResolvedValue(reply(409, { error: "username_taken" })),
    );
    await expect(
      taken.acceptInvite({ token: "t", username: "u", password: "p" }),
    ).rejects.toMatchObject({ kind: "username_taken" });
    const invalid = createAuthClient(
      vi.fn().mockResolvedValue(reply(400, { error: "invalid", message: "password too short" })),
    );
    await expect(
      invalid.acceptInvite({ token: "t", username: "u", password: "p" }),
    ).rejects.toMatchObject({ kind: "invalid", message: "password too short" });
  });

  it("accepting posts the token with the chosen credentials", async () => {
    const f = vi.fn().mockResolvedValue(reply(204));
    await createAuthClient(f).acceptInvite({ token: "t", username: "carol", password: "pw" });
    expect(f.mock.calls[0]?.[0]).toBe("/api/v1/auth/invites/accept");
    expect(JSON.parse(String((f.mock.calls[0]?.[1] as RequestInit).body))).toEqual({
      token: "t",
      username: "carol",
      password: "pw",
    });
  });

  it("a password-change-required refusal and a plain 403 are told apart", async () => {
    const must = createAuthClient(
      vi.fn().mockResolvedValue(reply(403, { error: "password_change_required" })),
    );
    await expect(must.status()).rejects.toMatchObject({ kind: "password_change_required" });
    const nope = createAuthClient(vi.fn().mockResolvedValue(reply(403, { error: "forbidden" })));
    await expect(nope.status()).rejects.toMatchObject({ kind: "forbidden" });
  });
});

describe("session watching", () => {
  it("asks for a re-check when a data route says the password must change", async () => {
    const seen = vi.fn();
    const off = onRecheckRequested(seen);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(reply(403, { error: "password_change_required" })),
    );
    await apiFetch("/api/v1/devices/dev0/state");
    await vi.waitFor(() => expect(seen).toHaveBeenCalledTimes(1));
    // An ordinary refusal is left to the caller.
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(reply(403, { error: "forbidden" })));
    await apiFetch("/api/v1/devices/dev0/tune", { method: "POST" });
    await new Promise((r) => setTimeout(r, 10));
    expect(seen).toHaveBeenCalledTimes(1);
    expect(isSessionLost()).toBe(false);
    off();
  });

  it("a closed socket re-reads the status: kicked ends the session, still valid reports it", async () => {
    const seen = vi.fn();
    const off = onSessionStatus(seen);
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValue(
          reply(200, { setup_required: false, authenticated: true, role: "listener" }),
        ),
    );
    checkSessionAfterClose();
    await vi.waitFor(() => expect(seen).toHaveBeenCalledTimes(1));
    expect(isSessionLost()).toBe(false);
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue(reply(200, { setup_required: false, authenticated: false })),
    );
    checkSessionAfterClose();
    await vi.waitFor(() => expect(isSessionLost()).toBe(true));
    off();
  });
});
