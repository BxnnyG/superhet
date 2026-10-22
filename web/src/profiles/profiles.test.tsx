import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import { parseStateDoc, type StateDoc } from "../state/doc";
import { toasts } from "../ui";
import { buildProfileData, createProfileApi, parseProfile } from "./api";
import { plural, relativeTime } from "./format";
import { ProfilesPanel } from "./ProfilesPanel";

const data = (center: number, extra: Record<string, unknown> = {}) => ({
  v: 1,
  center_hz: center,
  sample_rate: 2_048_000,
  vfos: [{ freq_hz: center + 1000, mode: "nfm" }],
  lock: false,
  decoders: [{ keep: "me" }],
  ...extra,
});
const resource = (id: string, name: string, center = 433_920_000, extra = {}) => ({
  id,
  name,
  data: data(center, extra),
  created_at: "2026-01-01T00:00:00Z",
  updated_at: new Date(Date.now() - 5 * 60_000).toISOString(),
});

const rawState = (center: number, locks: unknown[] = []) => ({
  v: 1,
  device: "dev0",
  driver: "rtlsdr",
  center_hz: center,
  sample_rate: 2_048_000,
  gain: { mode: "manual", db: 28 },
  gains_db: [0, 28],
  ppm: 1,
  bias_tee: false,
  tuning_epoch: 1,
  locks,
  window: { start_hz: center - 870_000, end_hz: center + 870_000 },
  vfos: [
    {
      id: 7,
      freq_hz: center + 5000,
      mode: "wfm",
      bandwidth_hz: 180000,
      squelch_dbfs: -40,
      deemphasis: "50us",
      offset_hz: 5000,
      status: "active",
    },
    {
      id: 8,
      freq_hz: center + 9000,
      mode: "am",
      bandwidth_hz: 10000,
      squelch_dbfs: null,
      deemphasis: null,
      offset_hz: 9000,
      status: "active",
    },
  ],
});

interface Call {
  method: string;
  url: string;
  body: unknown;
}

/** Minimal fake of the profile routes on top of a mocked fetch. */
function fakeBackend(initial: ReturnType<typeof resource>[]) {
  let list = [...initial];
  const calls: Call[] = [];
  let loadReply: () => Response = () =>
    new Response(JSON.stringify({ v: 1, ok: true, state: rawState(433_920_000) }));
  const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
  const fetchImpl = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = String(input);
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    calls.push({ method, url, body });
    if (url === "/api/v1/profiles" && method === "GET") return json({ v: 1, profiles: list });
    if (url.endsWith("/profile/save")) {
      if (list.some((p) => p.name.toLowerCase() === body.name.toLowerCase())) {
        return json({ v: 1, ok: false, code: "conflict", error: "name taken" }, 409);
      }
      const p = resource(`id-${list.length + 1}`, body.name, 100e6);
      list = [...list, p];
      return json({ v: 1, profile: p }, 201);
    }
    if (url.endsWith("/profile/load")) return loadReply();
    const m = /^\/api\/v1\/profiles\/(.+)$/.exec(url);
    if (m && method === "PUT") {
      if (list.some((p) => p.id !== m[1] && p.name.toLowerCase() === body.name.toLowerCase())) {
        return json({ v: 1, ok: false, code: "conflict", error: "name taken" }, 409);
      }
      list = list.map((p) => (p.id === m[1] ? { ...p, name: body.name, data: body.data } : p));
      return json({ v: 1, profile: list.find((p) => p.id === m[1]) });
    }
    if (m && method === "DELETE") {
      list = list.filter((p) => p.id !== m[1]);
      return new Response(null, { status: 204 });
    }
    return json({ error: "nope" }, 404);
  });
  return {
    fetchImpl: fetchImpl as unknown as typeof fetch,
    calls,
    setLoad: (f: () => Response) => (loadReply = f),
    mutating: () => calls.filter((c) => c.method !== "GET"),
    /** A profile created behind the panel's back (another tab, the API). */
    addExternally: (p: ReturnType<typeof resource>) => (list = [...list, p]),
  };
}

const docOf = (center = 100e6, locks: unknown[] = []): StateDoc =>
  parseStateDoc(rawState(center, locks));
const userLock = [{ kind: "user", id: "user", since_ms: 1 }];

function setup(
  initial: ReturnType<typeof resource>[],
  opts: { doc?: StateDoc | null; onUnlock?: () => Promise<boolean> } = {},
) {
  const backend = fakeBackend(initial);
  const onState = vi.fn();
  const onUnlock = opts.onUnlock ?? vi.fn(async () => true);
  const api = createProfileApi(backend.fetchImpl);
  const panel = (extra: { connected?: boolean; refreshSignal?: unknown } = {}) => (
    <ProfilesPanel
      api={api}
      device="dev0"
      doc={opts.doc === undefined ? docOf() : opts.doc}
      onState={onState}
      onUnlock={onUnlock}
      {...extra}
    />
  );
  const { rerender } = render(panel());
  return {
    backend,
    onState,
    onUnlock,
    update: (extra: { connected?: boolean; refreshSignal?: unknown }) => rerender(panel(extra)),
  };
}

beforeEach(() => toasts.clear());
afterEach(() => toasts.clear());

describe("helpers", () => {
  it("formats relative times", () => {
    const now = Date.parse("2026-05-01T12:00:00Z");
    expect(relativeTime("2026-05-01T11:59:50Z", now)).toBe("just now");
    expect(relativeTime("2026-05-01T11:55:00Z", now)).toBe("5 min ago");
    expect(relativeTime("2026-05-01T09:00:00Z", now)).toBe("3 h ago");
    expect(relativeTime("2026-04-29T12:00:00Z", now)).toBe("2 d ago");
    expect(relativeTime("2025-01-01T00:00:00Z", now)).toBe("2025-01-01");
    expect(relativeTime("x", now)).toBe("");
    expect(plural(1, "VFO")).toBe("1 VFO");
    expect(plural(0, "VFO")).toBe("0 VFOs");
  });

  it("parses a profile resource and rejects broken ones", () => {
    const p = parseProfile(resource("a", "Air", 118e6));
    expect(p).toMatchObject({ id: "a", name: "Air", centerHz: 118e6, vfoCount: 1, lock: false });
    expect(() => parseProfile({ id: "a" })).toThrow(ApiError);
  });

  it("builds a profile document from the device state, keeping stored decoders", () => {
    const d = buildProfileData(docOf(100e6, userLock), { decoders: [{ keep: "me" }] });
    expect(d).toMatchObject({
      v: 1,
      center_hz: 100e6,
      sample_rate: 2_048_000,
      gain: { mode: "manual", db: 28 },
      lock: true,
      decoders: [{ keep: "me" }],
    });
    const vfos = d.vfos as Record<string, unknown>[];
    expect(vfos[0]).toEqual({
      freq_hz: 100_005_000,
      mode: "wfm",
      bandwidth_hz: 180000,
      squelch_dbfs: -40,
      deemphasis: "50us",
    });
    expect(vfos[1]).toEqual({ freq_hz: 100_009_000, mode: "am", bandwidth_hz: 10000 });
  });
});

describe("ProfilesPanel", () => {
  it("shows an empty state", async () => {
    setup([]);
    expect(await screen.findByText("No profiles yet")).toBeInTheDocument();
  });

  describe("refreshing the list", () => {
    const external = () => resource("ext", "From elsewhere");
    const listReads = (b: ReturnType<typeof fakeBackend>) =>
      b.calls.filter((c) => c.method === "GET").length;

    it("picks up a profile saved elsewhere when the panel is shown again", async () => {
      const t = setup([]);
      await screen.findByText("No profiles yet");
      t.backend.addExternally(external());
      t.update({ refreshSignal: "true:false" });
      expect(await screen.findByText("From elsewhere")).toBeInTheDocument();
    });

    it("picks up a profile saved elsewhere when the window regains focus", async () => {
      const t = setup([]);
      await screen.findByText("No profiles yet");
      t.backend.addExternally(external());
      fireEvent(window, new Event("focus"));
      expect(await screen.findByText("From elsewhere")).toBeInTheDocument();
    });

    it("re-reads when the tab becomes visible again", async () => {
      const t = setup([]);
      await screen.findByText("No profiles yet");
      t.backend.addExternally(external());
      const before = listReads(t.backend);
      fireEvent(document, new Event("visibilitychange"));
      expect(await screen.findByText("From elsewhere")).toBeInTheDocument();
      expect(listReads(t.backend)).toBeGreaterThan(before);
    });

    it("re-reads after the state feed reconnects", async () => {
      const t = setup([]);
      await screen.findByText("No profiles yet");
      t.update({ connected: false });
      t.backend.addExternally(external());
      t.update({ connected: true });
      expect(await screen.findByText("From elsewhere")).toBeInTheDocument();
    });
  });

  it("lists name, centre, VFO count and age", async () => {
    setup([resource("a", "Airband", 118_000_000), resource("b", "PMR", 446_000_000)]);
    const list = await screen.findByRole("list", { name: "Profile list" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent("Airband");
    expect(rows[0]).toHaveTextContent("118.000 MHz");
    expect(rows[0]).toHaveTextContent("2.048 MS/s");
    expect(rows[0]).toHaveTextContent("1 VFO");
    expect(rows[0]).toHaveTextContent("5 min ago");
  });

  it("shows an unavailable notice with retry when the list cannot be read", async () => {
    const fetchImpl = vi.fn(async () =>
      Response.json(
        { v: 1, ok: false, code: "unavailable", error: "no database" },
        { status: 503 },
      ),
    );
    render(
      <ProfilesPanel
        api={createProfileApi(fetchImpl as unknown as typeof fetch)}
        device="dev0"
        doc={docOf()}
        onState={() => undefined}
        onUnlock={async () => true}
      />,
    );
    expect(await screen.findByText(/Profiles are unavailable: no database/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(2));
  });

  it("loads a profile and hands the new state on", async () => {
    const { backend, onState } = setup([resource("a", "Home", 433_920_000)], {
      doc: docOf(433_920_000),
    });
    fireEvent.click(await screen.findByRole("button", { name: "Load profile Home" }));
    await waitFor(() => expect(onState).toHaveBeenCalledTimes(1));
    expect(backend.mutating()[0]).toMatchObject({
      method: "POST",
      url: "/api/v1/devices/dev0/profile/load",
      body: { id: "a" },
    });
    expect(onState.mock.calls[0]?.[0].vfos).toHaveLength(2);
    expect(toasts.getSnapshot()[0]).toMatchObject({ tone: "success", title: "Loaded “Home”" });
  });

  it("asks before loading a profile that would move a locked tuning (unlock and load)", async () => {
    const { backend, onState, onUnlock } = setup([resource("a", "Air", 118e6)], {
      doc: docOf(100e6, userLock),
    });
    fireEvent.click(await screen.findByRole("button", { name: "Load profile Air" }));
    const dialog = await screen.findByRole("dialog", { name: "Tuning is locked" });
    expect(dialog).toHaveTextContent("Locked by you");
    expect(dialog).toHaveTextContent("118.000 MHz");
    expect(backend.mutating()).toHaveLength(0);
    fireEvent.click(within(dialog).getByRole("button", { name: "Unlock and load" }));
    await waitFor(() => expect(onState).toHaveBeenCalled());
    expect(onUnlock).toHaveBeenCalled();
    expect(backend.mutating()[0]?.url).toBe("/api/v1/devices/dev0/profile/load");
  });

  it("only informs when other holders keep the lock", async () => {
    const rec = [...userLock, { kind: "recording", id: "r", since_ms: 2 }];
    setup([resource("a", "Air", 118e6)], { doc: docOf(100e6, rec) });
    fireEvent.click(await screen.findByRole("button", { name: "Load profile Air" }));
    const dialog = await screen.findByRole("dialog", { name: "Tuning is locked" });
    expect(dialog).toHaveTextContent("Locked by you and a recording");
    expect(within(dialog).queryByRole("button", { name: "Unlock and load" })).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("loads straight through when the lock does not matter (same centre and rate)", async () => {
    const { onState } = setup([resource("a", "Same", 100e6)], { doc: docOf(100e6, userLock) });
    fireEvent.click(await screen.findByRole("button", { name: "Load profile Same" }));
    await waitFor(() => expect(onState).toHaveBeenCalled());
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the 409 reason when the server refuses a load (stale lock info)", async () => {
    const { backend, onState } = setup([resource("a", "Air", 118e6)], { doc: docOf(100e6) });
    backend.setLoad(() =>
      Response.json(
        {
          v: 1,
          ok: false,
          code: "locked",
          error: "tuning is locked by: user",
          holders: [{ kind: "user", id: "user", since_ms: 1 }],
        },
        { status: 409 },
      ),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Load profile Air" }));
    await waitFor(() => expect(toasts.getSnapshot()).toHaveLength(1));
    expect(toasts.getSnapshot()[0]).toMatchObject({ tone: "warn", title: "Tuning is locked" });
    expect(toasts.getSnapshot()[0]?.body).toContain("Locked by you");
    expect(onState).not.toHaveBeenCalled();
  });

  it("saves the current receiver under a new name", async () => {
    const { backend } = setup([]);
    await screen.findByText("No profiles yet");
    fireEvent.click(screen.getByRole("button", { name: /Save current as/ }));
    const dialog = screen.getByRole("dialog", { name: "Save current as…" });
    fireEvent.change(within(dialog).getByLabelText("Profile name"), {
      target: { value: " Mine " },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(backend.mutating()[0]).toMatchObject({
      url: "/api/v1/devices/dev0/profile/save",
      body: { name: "Mine" },
    });
    expect(await screen.findByText("Mine")).toBeInTheDocument();
  });

  it("requires a name and disables saving without a known state", async () => {
    setup([]);
    await screen.findByText("No profiles yet");
    fireEvent.click(screen.getByRole("button", { name: /Save current as/ }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Enter a name.")).toBeInTheDocument();
  });

  it("disables 'Save current as…' while the state is unknown", async () => {
    setup([], { doc: null });
    expect(screen.getByRole("button", { name: /Save current as/ })).toBeDisabled();
  });

  it("asks before overwriting a profile with the same name", async () => {
    const { backend } = setup([resource("a", "Home", 433e6, { decoders: [{ keep: "me" }] })]);
    await screen.findByText("Home");
    fireEvent.click(screen.getByRole("button", { name: /Save current as/ }));
    fireEvent.change(screen.getByLabelText("Profile name"), { target: { value: "home" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const confirm = await screen.findByRole("dialog", { name: "Overwrite profile?" });
    expect(confirm).toHaveTextContent("“Home” already exists");
    fireEvent.click(within(confirm).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const put = backend.mutating().find((c) => c.method === "PUT");
    expect(put?.url).toBe("/api/v1/profiles/a");
    expect(put?.body).toMatchObject({
      name: "Home",
      data: { center_hz: 100e6, decoders: [{ keep: "me" }] },
    });
  });

  it("cancelling the overwrite goes back to the name dialog", async () => {
    const { backend } = setup([resource("a", "Home")]);
    await screen.findByText("Home");
    fireEvent.click(screen.getByRole("button", { name: /Save current as/ }));
    fireEvent.change(screen.getByLabelText("Profile name"), { target: { value: "Home" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const confirm = await screen.findByRole("dialog", { name: "Overwrite profile?" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(screen.getByLabelText("Profile name")).toHaveValue("Home");
    expect(backend.mutating().filter((c) => c.method === "PUT")).toHaveLength(0);
  });

  it("renames a profile and reports a name clash inline", async () => {
    const { backend } = setup([resource("a", "Home"), resource("b", "Work")]);
    await screen.findByText("Home");
    fireEvent.click(screen.getByRole("button", { name: "Rename profile Home" }));
    const field = screen.getByLabelText("Profile name");
    expect(field).toHaveValue("Home");
    fireEvent.change(field, { target: { value: "work" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(await screen.findByText("A profile with this name already exists.")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Profile name"), { target: { value: "Cabin" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(
      backend
        .mutating()
        .filter((c) => c.method === "PUT")
        .pop()?.body,
    ).toMatchObject({
      name: "Cabin",
    });
    expect(await screen.findByText("Cabin")).toBeInTheDocument();
  });

  it("deletes only after confirmation", async () => {
    const { backend } = setup([resource("a", "Home")]);
    await screen.findByText("Home");
    fireEvent.click(screen.getByRole("button", { name: "Delete profile Home" }));
    let dialog = screen.getByRole("dialog", { name: "Delete profile?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(backend.mutating()).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Delete profile Home" }));
    dialog = screen.getByRole("dialog", { name: "Delete profile?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.getByText("No profiles yet")).toBeInTheDocument());
    expect(backend.mutating()[0]).toMatchObject({ method: "DELETE", url: "/api/v1/profiles/a" });
  });

  it("closes dialogs with Escape", async () => {
    setup([resource("a", "Home")]);
    await screen.findByText("Home");
    fireEvent.click(screen.getByRole("button", { name: "Delete profile Home" }));
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
