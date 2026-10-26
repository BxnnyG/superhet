import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_OVERLAY, OverlayStore } from "../bandplan/settings";
import { createBookmarkApi, parseBookmark } from "./api";
import { BookmarkEditorHost } from "./BookmarkEditor";
import { BookmarksPanel } from "./BookmarksPanel";
import { newInput } from "./model";
import { BookmarkStore } from "./store";

interface Raw {
  id: string;
  name: string;
  freq_hz: number;
  mode: string | null;
  bandwidth_hz: number | null;
  tags: string[];
  color: string | null;
  notes: string | null;
  squelch_db: number | null;
  squelch_auto: boolean;
  step_hz: number | null;
  tone_mode: string | null;
  ctcss_hz: number | null;
  dcs_code: number | null;
  dcs_polarity: string | null;
  digital: Record<string, unknown> | null;
  group_id: string | null;
  favourite: boolean;
  scan: { include: boolean; priority: number | null; dwell_ms: number | null };
  created_ms: number;
  updated_ms: number;
}

interface RawGroup {
  id: string;
  name: string;
  position: number;
}

const raw = (id: string, name: string, freq: number, extra: Partial<Raw> = {}): Raw => ({
  id,
  name,
  freq_hz: freq,
  mode: "nfm",
  bandwidth_hz: 12_500,
  tags: [],
  color: null,
  notes: null,
  squelch_db: null,
  squelch_auto: false,
  step_hz: null,
  tone_mode: null,
  ctcss_hz: null,
  dcs_code: null,
  dcs_polarity: null,
  digital: null,
  group_id: null,
  favourite: false,
  scan: { include: false, priority: null, dwell_ms: null },
  created_ms: 1,
  updated_ms: 1,
  ...extra,
});

interface Call {
  method: string;
  url: string;
  body: Record<string, unknown> | undefined;
}

/** In-memory stand-in for the bookmark REST routes. */
function fakeServer(
  initial: Raw[],
  fail?: (call: Call) => Response | undefined,
  initialGroups: RawGroup[] = [],
) {
  let items = [...initial];
  let groups = [...initialGroups];
  let next = 100;
  const calls: Call[] = [];
  const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
  const fetchFn = vi.fn((url: string, init?: RequestInit) => {
    const call: Call = {
      method: init?.method ?? "GET",
      url,
      body: init?.body ? (JSON.parse(String(init.body)) as Record<string, unknown>) : undefined,
    };
    calls.push(call);
    const failure = fail?.(call);
    if (failure) return Promise.resolve(failure);
    if (url === "/api/v1/bookmark-groups" && call.method === "GET") {
      return Promise.resolve(json({ v: 1, groups }));
    }
    if (url === "/api/v1/bookmark-groups" && call.method === "POST") {
      const created = { id: `g${next++}`, name: String(call.body?.name), position: groups.length };
      groups.push(created);
      return Promise.resolve(json(created, 201));
    }
    if (url === "/api/v1/bookmark-groups/order") {
      const ids = (call.body?.ids ?? []) as string[];
      groups = ids.map((gid, position) => ({
        ...(groups.find((g) => g.id === gid) as RawGroup),
        position,
      }));
      return Promise.resolve(json({ v: 1, groups }));
    }
    const groupId = /^\/api\/v1\/bookmark-groups\/([^/]+)$/.exec(url)?.[1];
    if (groupId && call.method === "PATCH") {
      groups = groups.map((g) => (g.id === groupId ? { ...g, name: String(call.body?.name) } : g));
      return Promise.resolve(json(groups.find((g) => g.id === groupId)));
    }
    if (groupId && call.method === "DELETE") {
      groups = groups.filter((g) => g.id !== groupId);
      items = items.map((b) => (b.group_id === groupId ? { ...b, group_id: null } : b));
      return Promise.resolve(new Response(null, { status: 204 }));
    }
    const id = /^\/api\/v1\/bookmarks\/([^/]+)$/.exec(url)?.[1];
    if (url === "/api/v1/bookmarks" && call.method === "GET") {
      return Promise.resolve(json({ v: 1, bookmarks: items }));
    }
    if (url === "/api/v1/bookmarks" && call.method === "POST") {
      const created = { ...raw(String(next++), "", 0), ...call.body } as Raw;
      items.push(created);
      return Promise.resolve(json(created, 201));
    }
    if (url === "/api/v1/bookmarks/bulk") {
      const list = (call.body?.bookmarks ?? []) as Partial<Raw>[];
      if (call.body?.replace === true) items = [];
      for (const b of list) items.push({ ...raw(String(next++), "", 0), ...b } as Raw);
      return Promise.resolve(json({ v: 1, created: list.length }, 201));
    }
    if (id && call.method === "PATCH") {
      items = items.map((b) => (b.id === id ? ({ ...b, ...call.body } as Raw) : b));
      return Promise.resolve(json(items.find((b) => b.id === id)));
    }
    if (id && call.method === "DELETE") {
      items = items.filter((b) => b.id !== id);
      return Promise.resolve(new Response(null, { status: 204 }));
    }
    return Promise.resolve(json({ v: 1, ok: false, code: "not_found", error: "no route" }, 404));
  });
  return { fetchFn, calls, items: () => items, groups: () => groups };
}

function setup(server: ReturnType<typeof fakeServer>, active: number | null = null) {
  const store = new BookmarkStore(createBookmarkApi(server.fetchFn as unknown as typeof fetch));
  const overlay = new OverlayStore(DEFAULT_OVERLAY, null);
  const onTune = vi.fn();
  const onTuneNew = vi.fn();
  const onAdd = vi.fn(() =>
    store.openEditor({
      kind: "create",
      draft: newInput({
        name: "PMR446 ch 3",
        freqHz: 446_031_250,
        mode: "nfm",
        bandwidthHz: 12_500,
      }),
    }),
  );
  render(
    <>
      <BookmarksPanel
        store={store}
        overlay={overlay}
        activeFreqHz={active}
        onTune={onTune}
        onTuneNew={onTuneNew}
        onAdd={onAdd}
      />
      <BookmarkEditorHost store={store} />
    </>,
  );
  void store.refresh();
  return { store, overlay, onTune, onTuneNew, onAdd };
}

const SEED = [
  raw("a", "Repeater DB0XY", 438_650_000, { tags: ["ham", "local"] }),
  raw("b", "Radio X", 100_100_000, { mode: "wfm", bandwidth_hz: 180_000, tags: ["broadcast"] }),
  raw("c", "Tower", 118_300_000, { mode: "am", bandwidth_hz: null, notes: "approach" }),
];

beforeEach(() => {
  window.localStorage.clear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("bookmark api", () => {
  it("parses a bookmark and tolerates numeric ids and unknown modes", () => {
    const b = parseBookmark({ ...raw("x", "N", 1e8), id: 7, mode: "dsb" });
    expect(b).toMatchObject({ id: "7", mode: null, freqHz: 1e8 });
    expect(() => parseBookmark({ name: "x" })).toThrow();
  });

  it("surfaces server errors with their message", async () => {
    const server = fakeServer(
      [],
      () =>
        new Response(
          JSON.stringify({ v: 1, ok: false, code: "bad_request", error: "name too long" }),
          {
            status: 400,
          },
        ),
    );
    const api = createBookmarkApi(server.fetchFn as unknown as typeof fetch);
    await expect(api.list()).rejects.toThrow("name too long");
  });
});

describe("BookmarksPanel", () => {
  it("lists bookmarks by frequency and tunes on click", async () => {
    const { onTune } = setup(fakeServer(SEED));
    const list = await screen.findByRole("list", { name: "Bookmark list" });
    const names = within(list)
      .getAllByRole("button", { name: /^Tune to/ })
      .map((b) => b.getAttribute("aria-label"));
    expect(names).toEqual(["Tune to Radio X", "Tune to Tower", "Tune to Repeater DB0XY"]);
    fireEvent.click(screen.getByRole("button", { name: "Tune to Repeater DB0XY" }));
    expect(onTune).toHaveBeenCalledWith({
      freqHz: 438_650_000,
      mode: "nfm",
      bandwidthHz: 12_500,
      toneSquelchHz: null,
    });
    fireEvent.click(screen.getByRole("button", { name: "Tune to Tower" }));
    expect(onTune).toHaveBeenLastCalledWith({
      freqHz: 118_300_000,
      mode: "am",
      toneSquelchHz: null,
    });
  });

  it("highlights the bookmark of the active frequency", async () => {
    setup(fakeServer(SEED), 100_100_000);
    // Radio X is also listed under "Near current frequency", so it appears twice.
    await screen.findAllByRole("button", { name: "Tune to Radio X" });
    for (const row of screen.getAllByRole("button", { name: "Tune to Radio X" })) {
      expect(row).toHaveAttribute("aria-pressed", "true");
    }
    expect(screen.getByRole("button", { name: "Tune to Tower" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("searches and filters by tag", async () => {
    setup(fakeServer(SEED));
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.change(screen.getByLabelText("Search bookmarks"), { target: { value: "tower" } });
    expect(screen.queryByRole("button", { name: "Tune to Radio X" })).toBeNull();
    expect(screen.getByRole("button", { name: "Tune to Tower" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Search bookmarks"), { target: { value: "" } });
    fireEvent.change(screen.getByLabelText("Tag"), { target: { value: "ham" } });
    expect(screen.getAllByRole("button", { name: /^Tune to/ })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Tune to Repeater DB0XY" })).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Search bookmarks"), { target: { value: "nothing" } });
    expect(screen.getByText("No bookmark matches.")).toBeInTheDocument();
  });

  it("shows an empty state", async () => {
    setup(fakeServer([]));
    expect(await screen.findByText("No bookmarks yet")).toBeInTheDocument();
  });

  it("edits a bookmark through the dialog and saves with PATCH", async () => {
    const server = fakeServer(SEED);
    setup(server);
    fireEvent.click(await screen.findByRole("button", { name: "Edit bookmark Radio X" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit bookmark" });
    const name = within(dialog).getByLabelText("Name");
    expect(name).toHaveValue("Radio X");
    expect(within(dialog).getByLabelText("Frequency (MHz)")).toHaveValue("100.1");
    fireEvent.change(name, { target: { value: "Radio Y" } });
    fireEvent.change(within(dialog).getByLabelText("Frequency (MHz)"), {
      target: { value: "100.2" },
    });
    fireEvent.change(within(dialog).getByLabelText("Tags"), { target: { value: "fm, news" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const patch = server.calls.find((c) => c.method === "PATCH");
    expect(patch?.url).toBe("/api/v1/bookmarks/b");
    expect(patch?.body).toMatchObject({
      name: "Radio Y",
      freq_hz: 100_200_000,
      mode: "wfm",
      bandwidth_hz: 180_000,
      tags: ["fm", "news"],
    });
    expect(await screen.findByRole("button", { name: "Tune to Radio Y" })).toBeInTheDocument();
  });

  it("adds a bookmark from the draft and posts it", async () => {
    const server = fakeServer(SEED);
    const { onAdd } = setup(server);
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.click(screen.getByRole("button", { name: "Add bookmark…" }));
    expect(onAdd).toHaveBeenCalled();
    const dialog = await screen.findByRole("dialog", { name: "Add bookmark" });
    expect(within(dialog).getByLabelText("Name")).toHaveValue("PMR446 ch 3");
    expect(within(dialog).getByLabelText("Bandwidth (kHz)")).toHaveValue("12.5");
    fireEvent.change(within(dialog).getByLabelText("Notes"), { target: { value: "family" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const post = server.calls.find((c) => c.method === "POST");
    // The server rejects unknown fields, so the body must not carry a version tag.
    expect(post?.body).not.toHaveProperty("v");
    expect(post?.body).toMatchObject({
      name: "PMR446 ch 3",
      freq_hz: 446_031_250,
      mode: "nfm",
      bandwidth_hz: 12_500,
      notes: "family",
    });
    expect(await screen.findByRole("button", { name: "Tune to PMR446 ch 3" })).toBeInTheDocument();
  });

  it("validates the dialog before sending anything", async () => {
    const server = fakeServer(SEED);
    setup(server);
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.click(screen.getByRole("button", { name: "Add bookmark…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add bookmark" });
    fireEvent.change(within(dialog).getByLabelText("Name"), { target: { value: "  " } });
    fireEvent.change(within(dialog).getByLabelText("Frequency (MHz)"), {
      target: { value: "abc" },
    });
    fireEvent.change(within(dialog).getByLabelText("Bandwidth (kHz)"), { target: { value: "-3" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByText("Enter a name.")).toBeInTheDocument();
    expect(within(dialog).getByText(/Enter a frequency/)).toBeInTheDocument();
    expect(within(dialog).getByText(/Enter kHz/)).toBeInTheDocument();
    expect(server.calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("keeps the dialog open and shows the server error when saving fails", async () => {
    const server = fakeServer(SEED, (c) =>
      c.method === "POST"
        ? new Response(
            JSON.stringify({ v: 1, ok: false, code: "conflict", error: "already exists" }),
            {
              status: 409,
            },
          )
        : undefined,
    );
    setup(server);
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.click(screen.getByRole("button", { name: "Add bookmark…" }));
    const dialog = await screen.findByRole("dialog", { name: "Add bookmark" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    expect(await within(dialog).findByText("already exists")).toBeInTheDocument();
  });

  it("deletes after confirmation", async () => {
    const server = fakeServer(SEED);
    setup(server);
    fireEvent.click(await screen.findByRole("button", { name: "Delete bookmark Tower" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete bookmark?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Tune to Tower" })).toBeNull());
    expect(server.calls.find((c) => c.method === "DELETE")?.url).toBe("/api/v1/bookmarks/c");
  });

  it("toggles the spectrum overlay for bookmarks", async () => {
    const { overlay } = setup(fakeServer(SEED));
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.click(screen.getByRole("button", { name: "More bookmark actions" }));
    fireEvent.click(screen.getByRole("menuitemcheckbox", { name: /Show on spectrum/ }));
    expect(overlay.getSnapshot().bookmarks).toBe(false);
  });
});

describe("import", () => {
  const CSV = `# Frequency ; Name ; Modulation ; Bandwidth ; Tags
  145800000 ; ISS ; Narrow FM ; 12500 ; Amateur
  100100000 ; Radio X ; WFM (stereo) ; 160000 ;
  438650000 ; Repeater DB0XY ; Narrow FM ; 12500 ;
`;

  async function choose(text: string, name = "bookmarks.csv") {
    await screen.findByRole("list", { name: "Bookmark list" });
    const input = screen.getByLabelText("Bookmark file to import");
    fireEvent.change(input, { target: { files: [new File([text], name)] } });
  }

  it("previews the counts and merges only new bookmarks", async () => {
    const server = fakeServer(SEED);
    setup(server);
    await choose(CSV);
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    expect(within(dialog).getByText(/3 bookmarks found/)).toBeInTheDocument();
    expect(within(dialog).getByText(/1 new, 2 already in your list/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Import 1" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const bulk = server.calls.find((c) => c.url === "/api/v1/bookmarks/bulk");
    expect(bulk?.body?.replace).toBe(false);
    expect(bulk?.body?.bookmarks).toHaveLength(1);
    expect(await screen.findByRole("button", { name: "Tune to ISS" })).toBeInTheDocument();
    expect(server.items()).toHaveLength(4);
  });

  it("replaces everything on request", async () => {
    const server = fakeServer(SEED);
    setup(server);
    await choose(CSV);
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    fireEvent.click(within(dialog).getByRole("radio", { name: "Replace all" }));
    expect(within(dialog).getByText(/Deletes your 3 current bookmarks/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Replace and import" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const bulk = server.calls.find((c) => c.url === "/api/v1/bookmarks/bulk");
    expect(bulk?.body?.replace).toBe(true);
    expect(server.items()).toHaveLength(3);
    expect(screen.queryByRole("button", { name: "Tune to Tower" })).toBeNull();
  });

  it("explains an unreadable file", async () => {
    setup(fakeServer(SEED));
    await choose("just some text", "notes.txt");
    expect(await screen.findByText(/Unrecognised file/)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("bookmarks v2 panel", () => {
  const GROUPS: RawGroup[] = [
    { id: "g1", name: "Airband", position: 0 },
    { id: "g2", name: "Ham", position: 1 },
  ];
  const V2 = [
    raw("a", "Repeater DB0XY", 438_650_000, {
      group_id: "g2",
      tone_mode: "ctcss",
      ctcss_hz: 88.5,
      squelch_db: -80,
      step_hz: 12_500,
      favourite: true,
    }),
    raw("b", "Tower", 118_300_000, { mode: "am", group_id: "g1" }),
    raw("c", "Dispatch", 392_000_000, {
      tone_mode: "dcs",
      dcs_code: 23,
      dcs_polarity: "R",
      digital: { protocol: "tetra", timeslot: 3, talkgroup: 1001 },
    }),
    raw("d", "ATIS", 118_700_000, { mode: "am", group_id: "g1" }),
  ];

  it("groups the list in folder order and folds a group", async () => {
    setup(fakeServer(V2, undefined, GROUPS));
    const list = await screen.findByRole("list", { name: "Bookmark list" });
    const heads = within(list)
      .getAllByRole("button", { expanded: true })
      .map((b) => b.textContent);
    expect(heads).toEqual(["Airband2", "Ham1", "Ungrouped1"]);
    fireEvent.click(within(list).getByRole("button", { name: /^Airband/ }));
    expect(screen.queryByRole("button", { name: "Tune to Tower" })).toBeNull();
    expect(screen.getByRole("button", { name: "Tune to Repeater DB0XY" })).toBeInTheDocument();
  });

  it("shows tone and digital badges", async () => {
    setup(fakeServer(V2, undefined, GROUPS));
    await screen.findByRole("list", { name: "Bookmark list" });
    expect(screen.getByText("88.5")).toBeInTheDocument();
    expect(screen.getByText("DCS 023R")).toBeInTheDocument();
    expect(screen.getByText("TETRA TS3 TG 1001")).toBeInTheDocument();
  });

  it("applies the whole stored state on click and opens a new VFO on alt-click", async () => {
    const { onTune, onTuneNew } = setup(fakeServer(V2, undefined, GROUPS));
    const row = await screen.findByRole("button", { name: "Tune to Repeater DB0XY" });
    fireEvent.click(row);
    expect(onTune).toHaveBeenCalledWith({
      freqHz: 438_650_000,
      mode: "nfm",
      bandwidthHz: 12_500,
      squelchDbfs: -80,
      stepHz: 12_500,
      toneSquelchHz: 88.5,
    });
    fireEvent.click(row, { altKey: true });
    expect(onTuneNew).toHaveBeenCalledTimes(1);
    expect(onTune).toHaveBeenCalledTimes(1);
    // DCS is stored only: it switches the CTCSS tone squelch off.
    fireEvent.click(screen.getByRole("button", { name: "Tune to Dispatch" }));
    expect(onTune).toHaveBeenLastCalledWith(expect.objectContaining({ toneSquelchHz: null }));
  });

  it("opens the editor on double-click", async () => {
    setup(fakeServer(V2, undefined, GROUPS));
    fireEvent.doubleClick(await screen.findByRole("button", { name: "Tune to Tower" }));
    expect(await screen.findByRole("dialog", { name: "Edit bookmark" })).toBeInTheDocument();
  });

  it("filters by group and favourites", async () => {
    setup(fakeServer(V2, undefined, GROUPS));
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.change(screen.getByLabelText("Group"), { target: { value: "g1" } });
    expect(screen.getAllByRole("button", { name: /^Tune to/ })).toHaveLength(2);
    fireEvent.change(screen.getByLabelText("Group"), { target: { value: "all" } });
    fireEvent.click(screen.getByRole("button", { name: "Favourites only" }));
    expect(screen.getAllByRole("button", { name: /^Tune to/ })).toHaveLength(1);
    expect(screen.getByRole("button", { name: "Tune to Repeater DB0XY" })).toBeInTheDocument();
  });

  it("lists the nearest bookmarks under the current frequency", async () => {
    setup(fakeServer(V2, undefined, GROUPS), 118_500_000);
    const list = await screen.findByRole("list", { name: "Bookmark list" });
    expect(within(list).getByText("Near current frequency")).toBeInTheDocument();
    // ATIS (200 kHz) and Tower (200 kHz) are near; each also appears in its folder.
    expect(within(list).getAllByRole("button", { name: "Tune to Tower" })).toHaveLength(2);
    expect(within(list).getAllByRole("button", { name: "Tune to Dispatch" })).toHaveLength(1);
  });

  it("renders only a window of a long list", async () => {
    const many = Array.from({ length: 600 }, (_, n) =>
      raw(`x${n}`, `Ch ${n}`, 100_000_000 + n * 1000),
    );
    setup(fakeServer(many));
    await screen.findByRole("list", { name: "Bookmark list" });
    const rendered = screen.getAllByRole("button", { name: /^Tune to/ }).length;
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(60);
  });

  it("manages groups: create and delete", async () => {
    const server = fakeServer(V2, undefined, GROUPS);
    setup(server);
    await screen.findByRole("list", { name: "Bookmark list" });
    fireEvent.click(screen.getByRole("button", { name: "More bookmark actions" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Manage groups…" }));
    const dialog = await screen.findByRole("dialog", { name: "Bookmark groups" });
    fireEvent.change(within(dialog).getByLabelText("New group"), { target: { value: "TETRA" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(server.groups().map((g) => g.name)).toContain("TETRA"));
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete group Ham" }));
    await waitFor(() => expect(server.groups().map((g) => g.name)).not.toContain("Ham"));
    expect(server.items().find((b) => b.id === "a")?.group_id).toBeNull();
  });

  it("imports groups by name and creates the missing ones first", async () => {
    const server = fakeServer([], undefined, GROUPS);
    setup(server);
    await screen.findByRole("list", { name: "Bookmark list" });
    const file = JSON.stringify({
      format: "superhet-bookmarks",
      v: 2,
      bookmarks: [
        { name: "A", freq_hz: 1e8, group: "airband" },
        { name: "B", freq_hz: 2e8, group: "New one" },
      ],
    });
    fireEvent.change(screen.getByLabelText("Bookmark file to import"), {
      target: { files: [new File([file], "x.json")] },
    });
    const dialog = await screen.findByRole("dialog", { name: "Import bookmarks" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Import 2" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(server.groups().map((g) => g.name)).toEqual(["Airband", "Ham", "New one"]);
    const byName = Object.fromEntries(server.items().map((b) => [b.name, b.group_id]));
    expect(byName.A).toBe("g1");
    expect(byName.B).toMatch(/^g/);
    expect(byName.B).not.toBe("g1");
  });

  it("saves the receiver state, sub-tone and digital channel from the editor", async () => {
    const server = fakeServer(V2, undefined, GROUPS);
    setup(server);
    fireEvent.click(await screen.findByRole("button", { name: "Edit bookmark Dispatch" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit bookmark" });
    expect(within(dialog).getByLabelText("DCS code")).toHaveValue("023");
    expect(within(dialog).getByText(/Store only/)).toBeInTheDocument();
    fireEvent.change(within(dialog).getByLabelText("Sub-tone"), { target: { value: "ctcss" } });
    fireEvent.change(within(dialog).getByLabelText("CTCSS tone"), { target: { value: "88.5" } });
    fireEvent.change(within(dialog).getByLabelText("Squelch"), { target: { value: "fixed" } });
    fireEvent.change(within(dialog).getByLabelText("Squelch level (dBFS)"), {
      target: { value: "-72" },
    });
    fireEvent.change(within(dialog).getByLabelText("Tuning step"), { target: { value: "25000" } });
    fireEvent.change(within(dialog).getByLabelText("Timeslot"), { target: { value: "4" } });
    fireEvent.change(within(dialog).getByLabelText("Group"), { target: { value: "g2" } });
    fireEvent.click(within(dialog).getByRole("switch", { name: "Include in scans" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    const patch = server.calls.find((c) => c.method === "PATCH");
    expect(patch?.url).toBe("/api/v1/bookmarks/c");
    expect(patch?.body).toMatchObject({
      tone_mode: "ctcss",
      ctcss_hz: 88.5,
      dcs_code: null,
      dcs_polarity: null,
      squelch_db: -72,
      squelch_auto: false,
      step_hz: 25_000,
      group_id: "g2",
      digital: { protocol: "tetra", timeslot: 4, talkgroup: 1001 },
      scan: { include: true },
    });
  });
});
