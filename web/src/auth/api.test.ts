import { afterEach, describe, expect, it, vi } from "vitest";
import { AuthError, createAuthClient, parseStatus } from "./api";
import { apiFetch, isSessionLost, onSessionLost, resetSession, verifySession } from "./session";
import { passwordStrength } from "./password";

const reply = (status: number, body?: unknown, headers: Record<string, string> = {}) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status, headers });

afterEach(() => vi.unstubAllGlobals());

describe("auth client", () => {
  it("parses the status document", () => {
    expect(parseStatus({ setup_required: false, authenticated: true, username: "a" })).toEqual({
      setupRequired: false,
      authenticated: true,
      username: "a",
      userId: null,
      role: "admin",
      kind: "user",
      mustChangePassword: false,
      guest: { enabled: false, privacyText: null },
    });
    expect(() => parseStatus({})).toThrow(AuthError);
  });

  it("posts credentials as JSON with same-origin credentials", async () => {
    const f = vi.fn().mockResolvedValue(reply(204));
    await createAuthClient(f).login({ username: "u", password: "p" });
    expect(f).toHaveBeenCalledWith(
      "/api/v1/auth/login",
      expect.objectContaining({
        method: "POST",
        credentials: "same-origin",
        body: JSON.stringify({ username: "u", password: "p" }),
      }),
    );
  });

  it("maps status codes to error kinds", async () => {
    const kind = async (res: Response) => {
      const c = createAuthClient(vi.fn().mockResolvedValue(res));
      return c.login({ username: "u", password: "p" }).catch((e: AuthError) => e);
    };
    expect(await kind(reply(401, { error: "invalid_credentials" }))).toMatchObject({
      kind: "invalid_credentials",
    });
    expect(await kind(reply(409, { error: "already_setup" }))).toMatchObject({
      kind: "already_setup",
    });
    expect(await kind(reply(403, { error: "invalid_setup_token" }))).toMatchObject({
      kind: "invalid_setup_token",
    });
    expect(await kind(reply(400, { error: "invalid", message: "bad" }))).toMatchObject({
      kind: "invalid",
      message: "bad",
    });
    expect(
      await kind(reply(429, { error: "rate_limited" }, { "Retry-After": "42" })),
    ).toMatchObject({ kind: "rate_limited", retryAfter: 42 });
    expect(await kind(reply(429))).toMatchObject({ kind: "rate_limited", retryAfter: 30 });
  });

  it("reports network failures", async () => {
    const c = createAuthClient(vi.fn().mockRejectedValue(new TypeError("fail")));
    await expect(c.status()).rejects.toMatchObject({ kind: "network" });
  });

  it("distinguishes a wrong current password from a lost session", async () => {
    const wrong = createAuthClient(
      vi.fn().mockResolvedValue(reply(401, { error: "invalid_credentials" })),
    );
    await expect(
      wrong.changePassword({ currentPassword: "a", newPassword: "b" }),
    ).rejects.toMatchObject({ kind: "invalid_credentials" });
    const gone = createAuthClient(
      vi.fn().mockResolvedValue(reply(401, { error: "unauthenticated" })),
    );
    await expect(
      gone.changePassword({ currentPassword: "a", newPassword: "b" }),
    ).rejects.toMatchObject({ kind: "unauthenticated" });
  });
});

describe("session", () => {
  it("marks the session lost on a 401 from the data API only", async () => {
    const listener = vi.fn();
    const off = onSessionLost(listener);
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(reply(401, { error: "unauthenticated" })));
    await apiFetch("/api/v1/auth/login", { method: "POST" });
    expect(isSessionLost()).toBe(false);
    await apiFetch("/api/v1/devices/dev0/state");
    expect(isSessionLost()).toBe(true);
    await apiFetch("/api/v1/devices/dev0/state");
    expect(listener).toHaveBeenCalledTimes(1);
    off();
    resetSession();
  });

  it("sends same-origin credentials", async () => {
    const f = vi.fn().mockResolvedValue(reply(200, {}));
    vi.stubGlobal("fetch", f);
    await apiFetch("/api/v1/devices/dev0/tune", { method: "POST" });
    expect(f).toHaveBeenCalledWith(
      "/api/v1/devices/dev0/tune",
      expect.objectContaining({ credentials: "same-origin", method: "POST" }),
    );
  });

  it("verifySession marks the session lost only when the server says signed out", async () => {
    await verifySession(
      vi.fn().mockResolvedValue(reply(200, { setup_required: false, authenticated: true })),
    );
    expect(isSessionLost()).toBe(false);
    await verifySession(vi.fn().mockRejectedValue(new TypeError("down")));
    expect(isSessionLost()).toBe(false);
    await verifySession(
      vi.fn().mockResolvedValue(reply(200, { setup_required: false, authenticated: false })),
    );
    expect(isSessionLost()).toBe(true);
  });
});

describe("passwordStrength", () => {
  it("rates by length and variety", () => {
    expect(passwordStrength("").label).toBe("");
    expect(passwordStrength("short").score).toBe(0);
    expect(passwordStrength("aaaaaaaaaaaaaaaaaaaaaaaa").score).toBe(1);
    expect(passwordStrength("abcdefghijkl").score).toBe(1);
    expect(passwordStrength("correct horse battery staple 42").score).toBe(4);
  });
});
