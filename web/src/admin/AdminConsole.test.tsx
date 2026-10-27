import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { Root } from "../Root";
import { AuthContext } from "../auth/context";
import { accountStatus, authValue } from "../auth/testing";
import { toasts } from "../ui";
import {
  DEFAULT_GUEST_POLICY,
  type AdminApi,
  type AuditEvent,
  type Invite,
  type LiveSession,
  type User,
} from "./api";
import AdminConsole from "./AdminConsole";

const NOW = Date.now();

const user = (id: number, over: Partial<User> = {}): User => ({
  id,
  username: `user${id}`,
  role: "listener",
  disabled: false,
  mustChangePassword: false,
  createdAtMs: NOW - 86_400_000,
  lastLoginMs: NOW - 3_600_000,
  sessions: 1,
  ...over,
});

const USERS = [
  user(1, { username: "root", role: "admin" }),
  user(2, { username: "olivia", role: "operator", mustChangePassword: true }),
  user(3, { username: "lena", role: "listener", disabled: true }),
];

const invite = (id: number, over: Partial<Invite> = {}): Invite => ({
  id,
  role: "listener",
  note: "",
  createdBy: "root",
  createdAtMs: NOW - 60_000,
  expiresAtMs: NOW + 3_600_000,
  state: "open",
  usedBy: null,
  ...over,
});

const SESSIONS: LiveSession[] = [
  {
    sid: "u.aa11",
    kind: "user",
    userId: 2,
    username: "olivia",
    role: "operator",
    ip: "198.51.100.7",
    userAgent: "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0",
    createdAtMs: NOW - 600_000,
    lastSeenMs: NOW - 5_000,
    idleSecs: null,
    connections: [
      { kind: "audio", device: "dev0", sinceMs: NOW - 500_000 },
      { kind: "spectrum", device: "dev0", sinceMs: NOW - 500_000 },
    ],
    vfos: [{ device: "dev0", id: 2, freqHz: 99_200_000, mode: "wfm", status: "active" }],
  },
  {
    sid: "g.bb22cc",
    kind: "guest",
    userId: null,
    username: null,
    role: "listener",
    ip: "203.0.113.0",
    userAgent: null,
    createdAtMs: NOW - 120_000,
    lastSeenMs: null,
    idleSecs: 14,
    connections: [],
    vfos: [],
  },
];

const event = (id: number, over: Partial<AuditEvent> = {}): AuditEvent => ({
  id,
  tsMs: NOW - id * 1000,
  actorKind: "user",
  actorId: "2",
  actorName: "olivia",
  role: "operator",
  ip: "198.51.100.7",
  userAgent: "UA",
  action: "device.tune",
  target: "dev0",
  outcome: "ok",
  details: { merged: 3 },
  ...over,
});

function fakeApi(over: Partial<AdminApi> = {}): AdminApi {
  return {
    listUsers: vi.fn().mockResolvedValue(USERS),
    createUser: vi.fn().mockResolvedValue({
      user: user(9, { username: "newbie" }),
      password: "generated one time password",
    }),
    updateUser: vi.fn().mockResolvedValue(USERS[1]),
    deleteUser: vi.fn().mockResolvedValue(undefined),
    resetPassword: vi.fn().mockResolvedValue("reset one time password"),
    listInvites: vi
      .fn()
      .mockResolvedValue([
        invite(1, { note: "for bob" }),
        invite(2, { state: "used", usedBy: "bob" }),
      ]),
    createInvite: vi
      .fn()
      .mockResolvedValue({ invite: invite(3), token: "tok-secret", path: "/invite/tok-secret" }),
    revokeInvite: vi.fn().mockResolvedValue(undefined),
    listSessions: vi.fn().mockResolvedValue(SESSIONS),
    kickSession: vi.fn().mockResolvedValue(undefined),
    listBans: vi.fn().mockResolvedValue([
      {
        id: 4,
        network: "203.0.113.7/32",
        reason: "spraying",
        state: "active",
        createdAtMs: NOW - 60_000,
        createdBy: "root",
        expiresAtMs: NOW + 3_600_000,
        liftedAtMs: null,
        liftedBy: null,
      },
    ]),
    createBan: vi.fn().mockResolvedValue({
      ban: {
        id: 5,
        network: "203.0.113.9/32",
        reason: null,
        state: "active",
        createdAtMs: NOW,
        createdBy: "root",
        expiresAtMs: null,
        liftedAtMs: null,
        liftedBy: null,
      },
      kicked: 1,
    }),
    liftBan: vi.fn().mockResolvedValue(undefined),
    audit: vi.fn().mockResolvedValue({
      events: [event(5), event(4, { outcome: "denied", action: "access.denied" })],
      nextBeforeId: 4,
    }),
    exportUrl: vi.fn(
      (f, format) => `/api/v1/admin/audit/export?format=${format}&n=${Object.keys(f).length}`,
    ),
    getGuestPolicy: vi
      .fn()
      .mockResolvedValue({ ...DEFAULT_GUEST_POLICY, privacyText: "Hello visitors" }),
    putGuestPolicy: vi.fn().mockImplementation((p) => Promise.resolve(p)),
    getAuditSettings: vi.fn().mockResolvedValue({
      settings: {
        retentionUserDays: 90,
        retentionGuestDays: 7,
        ipModeUser: "full",
        ipModeGuest: "truncated",
      },
      stats: { count: 1234, oldestMs: NOW - 5 * 86_400_000, bytes: 2_500_000 },
    }),
    putAuditSettings: vi.fn().mockImplementation((s) => Promise.resolve(s)),
    getLimits: vi.fn().mockResolvedValue({ listenerMaxVfos: 4, operatorMaxVfos: 0 }),
    putLimits: vi.fn().mockImplementation((l) => Promise.resolve(l)),
    ...over,
  };
}

function mount(section: string, api: AdminApi = fakeApi()) {
  window.history.replaceState(null, "", `/admin/${section}`);
  render(
    <AuthContext value={authValue(accountStatus("admin", "root", 1))}>
      <AdminConsole section={section} api={api} />
    </AuthContext>,
  );
  return api;
}

beforeEach(() => toasts.clear());
afterEach(() => {
  toasts.clear();
  window.history.replaceState(null, "", "/");
});

describe("console shell", () => {
  it("has a sidebar with every section and marks the current one", async () => {
    mount("users");
    const nav = screen.getByRole("navigation", { name: "Admin sections" });
    for (const name of [
      "Users and invites",
      "Live sessions",
      "Audit log",
      "Guest access",
      "Audit settings",
      "Limits",
    ]) {
      expect(within(nav).getByRole("link", { name })).toBeInTheDocument();
    }
    expect(within(nav).getByRole("link", { name: "Users and invites" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(within(nav).getByRole("link", { name: "Live sessions" })).not.toHaveAttribute(
      "aria-current",
    );
    expect(await screen.findByText("olivia")).toBeInTheDocument();
  });

  it("navigates between sections without a page load", async () => {
    mount("users");
    fireEvent.click(screen.getByRole("link", { name: "Limits" }));
    await waitFor(() => expect(window.location.pathname).toBe("/admin/limits"));
  });

  it("the bare /admin path canonicalises to the first section", async () => {
    mount("");
    await waitFor(() => expect(window.location.pathname).toBe("/admin/users"));
  });

  it("goes back to the receiver", () => {
    mount("users");
    fireEvent.click(screen.getByRole("button", { name: "Back to receiver" }));
    expect(window.location.pathname).toBe("/");
  });
});

describe("non-admins", () => {
  it("get an 'Admins only' page and no console requests", () => {
    window.history.replaceState(null, "", "/admin");
    render(
      <AuthContext value={authValue(accountStatus("operator", "olivia", 2))}>
        <Root />
      </AuthContext>,
    );
    expect(screen.getByRole("heading", { name: "Admins only" })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Admin sections" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Back to the receiver" }));
    expect(window.location.pathname).toBe("/");
  });
});

describe("users and invites", () => {
  it("lists accounts with role, state and sessions", async () => {
    mount("users");
    const olivia = await screen.findByTestId("user-olivia");
    expect(within(olivia).getByText("One-time password")).toBeInTheDocument();
    expect(within(olivia).getByRole("combobox", { name: "Role of olivia" })).toHaveValue(
      "operator",
    );
    expect(within(screen.getByTestId("user-lena")).getByText("Disabled")).toBeInTheDocument();
    expect(within(screen.getByTestId("user-root")).getByText("You")).toBeInTheDocument();
  });

  it("changes a role", async () => {
    const api = mount("users");
    await screen.findByTestId("user-olivia");
    fireEvent.change(screen.getByRole("combobox", { name: "Role of olivia" }), {
      target: { value: "admin" },
    });
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith(2, { role: "admin" }));
  });

  it("protects the last active admin and yourself", async () => {
    mount("users");
    await screen.findByTestId("user-root");
    expect(screen.getByRole("combobox", { name: "Role of root" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Actions for root" }));
    expect(screen.getByRole("menuitem", { name: "Disable account…" })).toBeDisabled();
    expect(screen.getByRole("menuitem", { name: "Delete account…" })).toBeDisabled();
    expect(screen.getByRole("menuitem", { name: "Reset password…" })).toBeEnabled();
  });

  it("creates a user and shows the generated one-time password once", async () => {
    const api = mount("users");
    await screen.findByTestId("user-olivia");
    fireEvent.click(screen.getByRole("button", { name: "New user" }));
    const dialog = screen.getByRole("dialog", { name: "New user" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
    expect(await within(dialog).findByText(/1 to 64 characters/)).toBeInTheDocument();
    expect(api.createUser).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText("Username"), { target: { value: "newbie" } });
    fireEvent.change(within(dialog).getByLabelText("Role"), { target: { value: "operator" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
    await waitFor(() =>
      expect(api.createUser).toHaveBeenCalledWith({ username: "newbie", role: "operator" }),
    );
    const secret = await screen.findByRole("dialog", { name: "One-time password" });
    expect(within(secret).getByLabelText("One-time password")).toHaveValue(
      "generated one time password",
    );
    fireEvent.click(within(secret).getByRole("button", { name: "Done" }));
    expect(screen.queryByText("generated one time password")).toBeNull();
  });

  it("shows a taken username on the field", async () => {
    const { HttpError } = await import("../http");
    const api = mount(
      "users",
      fakeApi({
        createUser: vi
          .fn()
          .mockRejectedValue(
            new HttpError(409, "username_taken", "This username is already taken."),
          ),
      }),
    );
    await screen.findByTestId("user-olivia");
    fireEvent.click(screen.getByRole("button", { name: "New user" }));
    const dialog = screen.getByRole("dialog", { name: "New user" });
    fireEvent.change(within(dialog).getByLabelText("Username"), { target: { value: "olivia" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create user" }));
    expect(await within(dialog).findByText("This username is already taken.")).toBeInTheDocument();
    expect(api.createUser).toHaveBeenCalled();
  });

  it("deletes after a confirmation", async () => {
    const api = mount("users");
    await screen.findByTestId("user-lena");
    fireEvent.click(screen.getByRole("button", { name: "Actions for lena" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Delete account…" }));
    expect(api.deleteUser).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog", { name: "Delete lena?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete account" }));
    await waitFor(() => expect(api.deleteUser).toHaveBeenCalledWith(3));
  });

  it("enables a disabled account directly and disables an active one after confirming", async () => {
    const api = mount("users");
    await screen.findByTestId("user-lena");
    fireEvent.click(screen.getByRole("button", { name: "Actions for lena" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Enable account" }));
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith(3, { disabled: false }));
    fireEvent.click(screen.getByRole("button", { name: "Actions for olivia" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Disable account…" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "Disable olivia?" })).getByRole("button", {
        name: "Disable account",
      }),
    );
    await waitFor(() => expect(api.updateUser).toHaveBeenCalledWith(2, { disabled: true }));
  });

  it("resets a password and shows the new one-time password", async () => {
    const api = mount("users");
    await screen.findByTestId("user-olivia");
    fireEvent.click(screen.getByRole("button", { name: "Actions for olivia" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Reset password…" }));
    const dialog = screen.getByRole("dialog", { name: "Reset password of olivia" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset password" }));
    await waitFor(() => expect(api.resetPassword).toHaveBeenCalledWith(2, undefined));
    expect(await screen.findByLabelText("One-time password")).toHaveValue(
      "reset one time password",
    );
  });

  it("creates an invite link with the origin and expiry, and offers copy", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const api = mount("users");
    await screen.findByTestId("user-olivia");
    fireEvent.click(screen.getByRole("button", { name: "Invite" }));
    const dialog = screen.getByRole("dialog", { name: "Invite someone" });
    fireEvent.change(within(dialog).getByLabelText("Role"), { target: { value: "operator" } });
    fireEvent.change(within(dialog).getByLabelText("Expires after"), { target: { value: "24" } });
    fireEvent.change(within(dialog).getByLabelText("Note (optional)"), {
      target: { value: "for carol" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create link" }));
    await waitFor(() =>
      expect(api.createInvite).toHaveBeenCalledWith({
        role: "operator",
        expiresInHours: 24,
        note: "for carol",
      }),
    );
    const link = await screen.findByRole("dialog", { name: "Invitation link" });
    const url = `${window.location.origin}/invite/tok-secret`;
    expect(within(link).getByLabelText(/Link for a new/)).toHaveValue(url);
    expect(within(link).getByText(/Expires/)).toBeInTheDocument();
    fireEvent.click(within(link).getByRole("button", { name: "Copy" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(url));
  });

  it("lists invitations and revokes an open one", async () => {
    const api = mount("users");
    const list = await screen.findByRole("list", { name: "Invitations" });
    expect(within(list).getByText("for bob")).toBeInTheDocument();
    expect(within(list).getByText("used")).toBeInTheDocument();
    expect(within(list).getAllByRole("button", { name: "Revoke" })).toHaveLength(1);
    fireEvent.click(within(list).getByRole("button", { name: "Revoke" }));
    fireEvent.click(
      within(screen.getByRole("dialog", { name: "Revoke this invitation?" })).getByRole("button", {
        name: "Revoke",
      }),
    );
    await waitFor(() => expect(api.revokeInvite).toHaveBeenCalledWith(1));
  });
});

describe("live sessions", () => {
  it("shows who, role, address, browser, connections and the tuned receivers", async () => {
    mount("sessions");
    const olivia = await screen.findByTestId("session-u.aa11");
    expect(within(olivia).getByText("Operator")).toBeInTheDocument();
    expect(within(olivia).getByText("198.51.100.7")).toBeInTheDocument();
    expect(within(olivia).getByText(/Firefox on Linux/)).toBeInTheDocument();
    expect(within(olivia).getByText("99.200 MHz")).toBeInTheDocument();
    expect(within(olivia).getByText(/WFM/)).toBeInTheDocument();
    expect(within(olivia).getByText("Audio")).toBeInTheDocument();
    const guest = screen.getByTestId("session-g.bb22cc");
    expect(within(guest).getByText("Guest bb22cc")).toBeInTheDocument();
    expect(within(guest).getByText(/idle 14 s/)).toBeInTheDocument();
    expect(within(guest).getByText("No open connection")).toBeInTheDocument();
  });

  it("kicks after a confirmation", async () => {
    const api = mount("sessions");
    const guest = await screen.findByTestId("session-g.bb22cc");
    fireEvent.click(within(guest).getByRole("button", { name: "Kick" }));
    expect(api.kickSession).not.toHaveBeenCalled();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Kick" }));
    await waitFor(() => expect(api.kickSession).toHaveBeenCalledWith("g.bb22cc"));
  });

  it("kicks and bans a session by its id, with a duration and a reason", async () => {
    const api = mount("sessions");
    const guest = await screen.findByTestId("session-g.bb22cc");
    fireEvent.click(within(guest).getByRole("button", { name: "Kick and ban" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Duration"), { target: { value: "60" } });
    fireEvent.change(within(dialog).getByLabelText("Reason (optional)"), {
      target: { value: "noise" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Kick and ban" }));
    await waitFor(() =>
      expect(api.createBan).toHaveBeenCalledWith({
        target: { session: "g.bb22cc" },
        kick: true,
        reason: "noise",
        expiresInMinutes: 60,
      }),
    );
  });

  it("refreshes on its own and can pause the refresh", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const api = mount("sessions");
      await screen.findByTestId("session-u.aa11");
      const calls = vi.mocked(api.listSessions).mock.calls.length;
      await vi.advanceTimersByTimeAsync(5100);
      expect(vi.mocked(api.listSessions).mock.calls.length).toBeGreaterThan(calls);
      fireEvent.click(screen.getByRole("switch", { name: "Auto-refresh" }));
      const paused = vi.mocked(api.listSessions).mock.calls.length;
      await vi.advanceTimersByTimeAsync(12_000);
      expect(vi.mocked(api.listSessions).mock.calls.length).toBe(paused);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("audit log", () => {
  it("loads the last 24 hours and shows actor, action, outcome and address", async () => {
    const api = mount("audit");
    const table = await screen.findByRole("table", { name: "Audit events" });
    expect(within(table).getAllByText("olivia")).toHaveLength(2);
    expect(within(table).getByText("access.denied")).toBeInTheDocument();
    expect(within(table).getByText("denied")).toBeInTheDocument();
    const [filter, page] = vi.mocked(api.audit).mock.calls[0] ?? [];
    expect(filter?.fromMs).toBeGreaterThan(NOW - 25 * 3_600_000);
    expect(page).toEqual({ limit: 100, beforeId: undefined });
  });

  it("applies filters", async () => {
    const api = mount("audit");
    await screen.findByRole("table", { name: "Audit events" });
    const form = screen.getByRole("form", { name: "Audit filters" });
    fireEvent.change(within(form).getByLabelText("Time range"), { target: { value: "all" } });
    fireEvent.change(within(form).getByLabelText("Actor"), { target: { value: "olivia" } });
    fireEvent.change(within(form).getByLabelText("Role"), { target: { value: "operator" } });
    fireEvent.change(within(form).getByLabelText("Action"), { target: { value: "device.*" } });
    fireEvent.change(within(form).getByLabelText("Outcome"), { target: { value: "denied" } });
    fireEvent.change(within(form).getByLabelText("IP address or network"), {
      target: { value: "198.51.100.0/24" },
    });
    fireEvent.click(within(form).getByRole("button", { name: /Apply filters/ }));
    await waitFor(() => expect(vi.mocked(api.audit).mock.calls.length).toBe(2));
    expect(vi.mocked(api.audit).mock.calls[1]?.[0]).toEqual({
      actor: "olivia",
      role: "operator",
      action: "device.*",
      outcome: "denied",
      ip: "198.51.100.0/24",
    });
  });

  it("pages with the cursor and goes back", async () => {
    const api = mount("audit");
    await screen.findByRole("table", { name: "Audit events" });
    expect(screen.getByRole("button", { name: "Newer" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Older" }));
    await waitFor(() => expect(vi.mocked(api.audit).mock.calls.length).toBe(2));
    expect(vi.mocked(api.audit).mock.calls[1]?.[1]).toEqual({ limit: 100, beforeId: 4 });
    await screen.findByText(/Page 2/);
    fireEvent.click(screen.getByRole("button", { name: "Newer" }));
    await screen.findByText(/Page 1/);
  });

  it("exports with the applied filters as CSV and JSON", async () => {
    mount("audit");
    await screen.findByRole("table", { name: "Audit events" });
    expect(screen.getByRole("link", { name: "Export CSV" })).toHaveAttribute(
      "href",
      expect.stringContaining("format=csv"),
    );
    expect(screen.getByRole("link", { name: "Export JSON" })).toHaveAttribute(
      "href",
      expect.stringContaining("format=json"),
    );
  });

  it("expands an event to its details", async () => {
    mount("audit");
    await screen.findByRole("table", { name: "Audit events" });
    fireEvent.click(screen.getByRole("button", { name: "Show details of event 5" }));
    expect(screen.getByText(/"merged": 3/)).toBeInTheDocument();
  });

  it("says so when nothing matches", async () => {
    mount(
      "audit",
      fakeApi({ audit: vi.fn().mockResolvedValue({ events: [], nextBeforeId: null }) }),
    );
    expect(await screen.findByText("No matching entries")).toBeInTheDocument();
  });

  it("shows an error with retry when the request fails", async () => {
    const { HttpError } = await import("../http");
    const audit = vi
      .fn()
      .mockRejectedValueOnce(new HttpError(500, "", "Request failed (HTTP 500)."))
      .mockResolvedValue({ events: [], nextBeforeId: null });
    mount("audit", fakeApi({ audit }));
    expect(await screen.findByText("Request failed (HTTP 500).")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No matching entries")).toBeInTheDocument();
  });
});

describe("guest access policy", () => {
  it("edits and saves the whole policy", async () => {
    const api = mount("guests");
    const save = await screen.findByRole("button", { name: "Save changes" });
    expect(save).toBeDisabled();
    fireEvent.click(screen.getByRole("switch", { name: "Allow guest access" }));
    fireEvent.click(
      within(screen.getByRole("group", { name: "Allowed modes" })).getByRole("button", {
        name: "USB",
      }),
    );
    fireEvent.click(screen.getByRole("switch", { name: "Only while the tuning is not locked" }));
    fireEvent.change(screen.getByLabelText("Notice text"), { target: { value: "Be nice." } });
    fireEvent.change(screen.getByLabelText("Receivers per guest"), { target: { value: "2" } });
    fireEvent.blur(screen.getByLabelText("Receivers per guest"));
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(api.putGuestPolicy).toHaveBeenCalled());
    expect(vi.mocked(api.putGuestPolicy).mock.calls[0]?.[0]).toMatchObject({
      enabled: true,
      allowedModes: ["wfm", "nfm", "am", "usb"],
      onlyWithoutLock: true,
      privacyText: "Be nice.",
      maxVfosPerGuest: 2,
      freqMinHz: null,
      freqMaxHz: null,
    });
    expect(await screen.findByText("All changes saved")).toBeInTheDocument();
  });

  it("limits the frequency range in MHz", async () => {
    const api = mount("guests");
    await screen.findByRole("button", { name: "Save changes" });
    fireEvent.click(screen.getByRole("switch", { name: "Limit the frequency range" }));
    const low = screen.getByLabelText("Lowest frequency");
    fireEvent.change(low, { target: { value: "144" } });
    fireEvent.blur(low);
    const high = screen.getByLabelText("Highest frequency");
    fireEvent.change(high, { target: { value: "146,5" } });
    fireEvent.blur(high);
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(api.putGuestPolicy).toHaveBeenCalled());
    expect(vi.mocked(api.putGuestPolicy).mock.calls[0]?.[0]).toMatchObject({
      freqMinHz: 144_000_000,
      freqMaxHz: 146_500_000,
    });
  });

  it("discards edits", async () => {
    const api = mount("guests");
    await screen.findByRole("button", { name: "Save changes" });
    fireEvent.click(screen.getByRole("switch", { name: "Allow guest access" }));
    expect(screen.getByRole("switch", { name: "Allow guest access" })).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.getByRole("switch", { name: "Allow guest access" })).not.toBeChecked();
    expect(api.putGuestPolicy).not.toHaveBeenCalled();
  });

  it("warns when no mode is allowed", async () => {
    mount("guests");
    await screen.findByRole("button", { name: "Save changes" });
    const modes = screen.getByRole("group", { name: "Allowed modes" });
    for (const m of ["WFM", "NFM", "AM"])
      fireEvent.click(within(modes).getByRole("button", { name: m }));
    expect(screen.getByText(/Allow at least one mode/)).toBeInTheDocument();
  });

  it("shows a server rejection", async () => {
    const { HttpError } = await import("../http");
    mount(
      "guests",
      fakeApi({
        putGuestPolicy: vi
          .fn()
          .mockRejectedValue(new HttpError(400, "invalid", "max_guests out of range")),
      }),
    );
    await screen.findByRole("button", { name: "Save changes" });
    fireEvent.click(screen.getByRole("switch", { name: "Allow guest access" }));
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    expect(await screen.findByText("max_guests out of range")).toBeInTheDocument();
  });
});

describe("audit settings and limits", () => {
  it("saves retention and address modes", async () => {
    const api = mount("audit-settings");
    expect(await screen.findByText(/^1[.,\s\u202f]?234$/)).toBeInTheDocument();
    expect(screen.getByText("2.4 MiB")).toBeInTheDocument();
    const days = screen.getByLabelText("Guest entries");
    fireEvent.change(days, { target: { value: "14" } });
    fireEvent.blur(days);
    fireEvent.change(screen.getByRole("combobox", { name: "Account addresses" }), {
      target: { value: "truncated" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() =>
      expect(api.putAuditSettings).toHaveBeenCalledWith({
        retentionUserDays: 90,
        retentionGuestDays: 14,
        ipModeUser: "truncated",
        ipModeGuest: "truncated",
      }),
    );
  });

  it("warns about keeping guest entries forever", async () => {
    mount("audit-settings");
    const days = await screen.findByLabelText("Guest entries");
    fireEvent.change(days, { target: { value: "0" } });
    fireEvent.blur(days);
    expect(screen.getByText(/discouraged/)).toBeInTheDocument();
  });

  it("saves the per-person receiver limits", async () => {
    const api = mount("limits");
    const listeners = await screen.findByLabelText("Listeners");
    fireEvent.change(listeners, { target: { value: "2" } });
    fireEvent.blur(listeners);
    const operators = screen.getByLabelText("Operators");
    fireEvent.change(operators, { target: { value: "6" } });
    fireEvent.blur(operators);
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() =>
      expect(api.putLimits).toHaveBeenCalledWith({ listenerMaxVfos: 2, operatorMaxVfos: 6 }),
    );
  });
});

describe("address bans", () => {
  it("lists active bans and lifts one after a confirmation", async () => {
    const api = mount("bans");
    const row = await screen.findByTestId("ban-4");
    expect(within(row).getByText("203.0.113.7/32")).toBeInTheDocument();
    expect(within(row).getByText(/spraying/)).toBeInTheDocument();
    fireEvent.click(within(row).getByRole("button", { name: "Lift" }));
    expect(api.liftBan).not.toHaveBeenCalled();
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Lift ban" }));
    await waitFor(() => expect(api.liftBan).toHaveBeenCalledWith(4));
  });

  it("bans a typed network and needs one", async () => {
    const api = mount("bans");
    await screen.findByTestId("ban-4");
    fireEvent.click(screen.getByRole("button", { name: "Ban an address" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Ban" }));
    expect(api.createBan).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText("Address or network"), {
      target: { value: "203.0.113.0/24" },
    });
    fireEvent.change(within(dialog).getByLabelText("Duration"), { target: { value: "0" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Ban" }));
    await waitFor(() =>
      expect(api.createBan).toHaveBeenCalledWith({
        target: { network: "203.0.113.0/24" },
        kick: true,
      }),
    );
  });

  it("offers to ban the address of an audit row", async () => {
    const api = mount("audit");
    fireEvent.click(await screen.findByRole("button", { name: "Show details of event 5" }));
    fireEvent.click(screen.getByRole("button", { name: /Ban 198\.51\.100\.7/ }));
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByLabelText("Address or network")).toHaveValue("198.51.100.7");
    fireEvent.click(within(dialog).getByRole("button", { name: "Ban" }));
    await waitFor(() => expect(api.createBan).toHaveBeenCalled());
  });
});
