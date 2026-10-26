import { describe, expect, it, vi } from "vitest";
import { HttpError } from "../http";
import {
  createAdminApi,
  createSelfApi,
  guestPolicyBody,
  parseAuditPage,
  parseGuestPolicy,
  parseLiveSession,
  parseUser,
} from "./api";

const reply = (status: number, body?: unknown, headers: Record<string, string> = {}) =>
  new Response(body === undefined ? null : JSON.stringify(body), { status, headers });

const USER = {
  id: 3,
  username: "alice",
  role: "operator",
  disabled: false,
  must_change_password: true,
  created_at_ms: 1000,
  last_login_ms: null,
  sessions: 2,
};

function api(res: Response | Response[]) {
  const queue = Array.isArray(res) ? [...res] : [res];
  const f = vi.fn(() =>
    Promise.resolve(
      queue.length > 1 ? (queue.shift() as Response) : (queue[0] as Response).clone(),
    ),
  );
  return { f, client: createAdminApi(f as unknown as typeof fetch) };
}

const call = (f: ReturnType<typeof vi.fn>, n = 0) => {
  const [url, init] = f.mock.calls[n] as unknown as [string, RequestInit];
  return { url, init, body: init.body ? (JSON.parse(String(init.body)) as unknown) : undefined };
};

describe("users and invites", () => {
  it("lists users", async () => {
    const { client } = api(reply(200, { users: [USER] }));
    expect(await client.listUsers()).toEqual([
      {
        id: 3,
        username: "alice",
        role: "operator",
        disabled: false,
        mustChangePassword: true,
        createdAtMs: 1000,
        lastLoginMs: null,
        sessions: 2,
      },
    ]);
  });

  it("creates a user and returns the generated password once", async () => {
    const { f, client } = api(reply(201, { user: USER, password: "one time password words" }));
    const r = await client.createUser({ username: "alice", role: "operator" });
    expect(r.password).toBe("one time password words");
    expect(r.user.username).toBe("alice");
    const c = call(f);
    expect(c.url).toBe("/api/v1/admin/users");
    expect(c.init.method).toBe("POST");
    expect(c.body).toEqual({ username: "alice", role: "operator" });
  });

  it("sends a chosen password and gets none back", async () => {
    const { f, client } = api(reply(201, { user: USER }));
    const r = await client.createUser({
      username: "alice",
      role: "listener",
      password: "twelve chars!",
    });
    expect(r.password).toBeNull();
    expect(call(f).body).toEqual({
      username: "alice",
      role: "listener",
      password: "twelve chars!",
    });
  });

  it("changes role and disabled state with PATCH", async () => {
    const { f, client } = api(reply(200, { user: { ...USER, role: "admin" } }));
    expect((await client.updateUser(3, { role: "admin" })).role).toBe("admin");
    expect(call(f)).toMatchObject({ url: "/api/v1/admin/users/3", body: { role: "admin" } });
    expect(call(f).init.method).toBe("PATCH");
  });

  it("deletes a user and resets a password", async () => {
    const del = api(reply(204));
    await del.client.deleteUser(3);
    expect(call(del.f).init.method).toBe("DELETE");
    const gen = api(reply(200, { password: "new-one-time-secret" }));
    expect(await gen.client.resetPassword(3)).toBe("new-one-time-secret");
    expect(call(gen.f).url).toBe("/api/v1/admin/users/3/password");
    const set = api(reply(204));
    expect(await set.client.resetPassword(3, "a chosen password")).toBeNull();
    expect(call(set.f).body).toEqual({ password: "a chosen password" });
  });

  it("maps the last-admin conflict", async () => {
    const { client } = api(reply(409, { error: "last_admin" }));
    await expect(client.updateUser(1, { role: "listener" })).rejects.toMatchObject({
      status: 409,
      code: "last_admin",
    });
    await expect(client.updateUser(1, { disabled: true })).rejects.toBeInstanceOf(HttpError);
  });

  it("creates an invite and keeps the token", async () => {
    const invite = {
      id: 9,
      role: "listener",
      note: "for bob",
      created_by: "benny",
      created_at_ms: 1,
      expires_at_ms: 2,
      state: "open",
      used_by: null,
    };
    const { f, client } = api(
      reply(201, { invite, token: "secret-token", path: "/invite/secret-token" }),
    );
    const r = await client.createInvite({ role: "listener", expiresInHours: 24, note: "for bob" });
    expect(r.token).toBe("secret-token");
    expect(r.path).toBe("/invite/secret-token");
    expect(r.invite).toMatchObject({ id: 9, state: "open", createdBy: "benny" });
    expect(call(f).body).toEqual({ role: "listener", expires_in_hours: 24, note: "for bob" });
  });

  it("revokes an invite", async () => {
    const { f, client } = api(reply(204));
    await client.revokeInvite(9);
    expect(call(f)).toMatchObject({ url: "/api/v1/admin/invites/9" });
    expect(call(f).init.method).toBe("DELETE");
  });
});

describe("live sessions", () => {
  const raw = {
    sid: "u.abcd",
    kind: "user",
    user_id: 3,
    username: "alice",
    role: "operator",
    ip: "198.51.100.7",
    user_agent: "Mozilla/5.0 Firefox/130.0",
    created_at_ms: 10,
    last_seen_ms: 20,
    connections: [{ kind: "audio", device: "dev0", since_ms: 11 }],
    vfos: [{ device: "dev0", id: 1, freq_hz: 100_100_000, mode: "wfm", status: "active" }],
  };

  it("parses accounts and guests", () => {
    expect(parseLiveSession(raw)).toMatchObject({
      sid: "u.abcd",
      username: "alice",
      vfos: [{ id: 1, freqHz: 100_100_000, mode: "wfm" }],
      connections: [{ kind: "audio" }],
    });
    const g = parseLiveSession({
      sid: "g.ff",
      kind: "guest",
      role: "listener",
      ip: "198.51.100.0",
      idle_secs: 42,
      created_at_ms: 1,
      connections: [],
      vfos: [],
    });
    expect(g).toMatchObject({ kind: "guest", userId: null, idleSecs: 42, ip: "198.51.100.0" });
  });

  it("kicks by sid", async () => {
    const { f, client } = api(reply(204));
    await client.kickSession("g.ff");
    expect(call(f).url).toBe("/api/v1/admin/sessions/g.ff");
    expect(call(f).init.method).toBe("DELETE");
  });

  it("lists", async () => {
    const { client } = api(reply(200, { sessions: [raw] }));
    expect(await client.listSessions()).toHaveLength(1);
  });
});

describe("audit", () => {
  const ev = {
    id: 7,
    ts_ms: 1_700_000_000_000,
    actor_kind: "user",
    actor_id: "3",
    actor_name: "alice",
    role: "operator",
    ip: null,
    user_agent: "UA",
    action: "device.tune",
    target: "dev0",
    outcome: "denied",
    details: { merged: 3 },
  };

  it("parses a page with its cursor", () => {
    const p = parseAuditPage({ events: [ev], next_before_id: 6 });
    expect(p.nextBeforeId).toBe(6);
    expect(p.events[0]).toMatchObject({
      id: 7,
      tsMs: 1_700_000_000_000,
      actorName: "alice",
      role: "operator",
      outcome: "denied",
      details: { merged: 3 },
    });
    expect(parseAuditPage({ events: [] }).nextBeforeId).toBeNull();
  });

  it("queries with filters and the paging cursor", async () => {
    const { f, client } = api(reply(200, { events: [ev], next_before_id: null }));
    await client.audit({ action: "device.*", outcome: "denied" }, { limit: 50, beforeId: 99 });
    const p = new URL(call(f).url, "http://x");
    expect(p.pathname).toBe("/api/v1/admin/audit");
    expect(Object.fromEntries(p.searchParams)).toEqual({
      action: "device.*",
      outcome: "denied",
      limit: "50",
      before_id: "99",
    });
  });

  it("builds export links for CSV and JSON with the same filters", () => {
    const { client } = api(reply(200));
    const csv = new URL(client.exportUrl({ role: "admin", fromMs: 5 }, "csv"), "http://x");
    expect(csv.pathname).toBe("/api/v1/admin/audit/export");
    expect(Object.fromEntries(csv.searchParams)).toEqual({
      role: "admin",
      from_ms: "5",
      format: "csv",
    });
    expect(new URL(client.exportUrl({}, "json"), "http://x").searchParams.get("format")).toBe(
      "json",
    );
  });
});

describe("settings", () => {
  const policy = {
    enabled: true,
    max_guests: 5,
    max_guests_per_ip: 2,
    max_vfos_per_guest: 1,
    allowed_modes: ["wfm", "nfm"],
    freq_min_hz: 88_000_000,
    freq_max_hz: null,
    idle_timeout_secs: 600,
    only_without_lock: true,
    show_decoder_events: false,
    privacy_text: "We keep your address for 7 days.",
  };

  it("round-trips the guest policy", () => {
    const p = parseGuestPolicy({ guest_policy: policy });
    expect(p).toMatchObject({
      enabled: true,
      maxVfosPerGuest: 1,
      allowedModes: ["wfm", "nfm"],
      freqMinHz: 88_000_000,
      freqMaxHz: null,
      onlyWithoutLock: true,
      privacyText: "We keep your address for 7 days.",
    });
    expect(guestPolicyBody(p)).toEqual(policy);
  });

  it("saves the guest policy with PUT", async () => {
    const { f, client } = api(reply(200, { guest_policy: policy }));
    const saved = await client.putGuestPolicy(parseGuestPolicy({ guest_policy: policy }));
    expect(saved.enabled).toBe(true);
    expect(call(f).init.method).toBe("PUT");
    expect(call(f).url).toBe("/api/v1/admin/guest-policy");
    expect(call(f).body).toEqual(policy);
  });

  it("reads and writes audit settings and limits", async () => {
    const settings = {
      audit_settings: {
        retention_user_days: 90,
        retention_guest_days: 7,
        ip_mode_user: "full",
        ip_mode_guest: "truncated",
      },
      stats: { count: 12, oldest_ms: 5, bytes: 4096 },
    };
    const a = api(reply(200, settings));
    const got = await a.client.getAuditSettings();
    expect(got.settings).toEqual({
      retentionUserDays: 90,
      retentionGuestDays: 7,
      ipModeUser: "full",
      ipModeGuest: "truncated",
    });
    expect(got.stats).toEqual({ count: 12, oldestMs: 5, bytes: 4096 });
    await a.client.putAuditSettings(got.settings);
    expect(call(a.f, 1).body).toEqual(settings.audit_settings);

    const l = api(reply(200, { limits: { listener_max_vfos: 4, operator_max_vfos: 0 } }));
    expect(await l.client.getLimits()).toEqual({ listenerMaxVfos: 4, operatorMaxVfos: 0 });
    await l.client.putLimits({ listenerMaxVfos: 2, operatorMaxVfos: 6 });
    expect(call(l.f, 1).body).toEqual({ listener_max_vfos: 2, operator_max_vfos: 6 });
  });

  it("a 403 for non-admins keeps its code", async () => {
    const { client } = api(reply(403, { error: "forbidden" }));
    await expect(client.getLimits()).rejects.toMatchObject({ status: 403, code: "forbidden" });
  });

  it("rejects replies of the wrong shape", async () => {
    const { client } = api(reply(200, { nonsense: true }));
    await expect(client.getLimits()).rejects.toBeInstanceOf(HttpError);
    expect(() => parseUser({ id: "x" })).toThrow(HttpError);
  });
});

describe("own sessions", () => {
  it("lists and revokes", async () => {
    const f = vi
      .fn()
      .mockResolvedValueOnce(
        reply(200, {
          sessions: [
            {
              sid: "s1",
              ip: "198.51.100.7",
              user_agent: "UA",
              created_at_ms: 1,
              last_seen_ms: 2,
              current: true,
            },
          ],
        }),
      )
      .mockResolvedValueOnce(reply(204));
    const self = createSelfApi(f as unknown as typeof fetch);
    const list = await self.list();
    expect(list[0]).toMatchObject({ sid: "s1", current: true, ip: "198.51.100.7" });
    await self.revoke("s1");
    expect(f.mock.calls[1]?.[0]).toBe("/api/v1/me/sessions/s1");
    expect((f.mock.calls[1]?.[1] as RequestInit).method).toBe("DELETE");
  });
});
