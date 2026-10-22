import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_OVERLAY, OverlayStore } from "../bandplan/settings";
import { createBookmarkApi, parseBookmark } from "./api";
import { BookmarkEditorHost } from "./BookmarkEditor";
import { BookmarksPanel } from "./BookmarksPanel";
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
  created_ms: number;
  updated_ms: number;
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
function fakeServer(initial: Raw[], fail?: (call: Call) => Response | undefined) {
  let items = [...initial];
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
  return { fetchFn, calls, items: () => items };
}

function setup(server: ReturnType<typeof fakeServer>, active: number | null = null) {
  const store = new BookmarkStore(createBookmarkApi(server.fetchFn as unknown as typeof fetch));
  const overlay = new OverlayStore(DEFAULT_OVERLAY, null);
  const onTune = vi.fn();
  const onAdd = vi.fn(() =>
    store.openEditor({
      kind: "create",
      draft: {
        name: "PMR446 ch 3",
        freqHz: 446_031_250,
        mode: "nfm",
        bandwidthHz: 12_500,
        tags: [],
        color: null,
        notes: null,
      },
    }),
  );
  render(
    <>
      <BookmarksPanel
        store={store}
        overlay={overlay}
        activeFreqHz={active}
        onTune={onTune}
        onAdd={onAdd}
      />
      <BookmarkEditorHost store={store} />
    </>,
  );
  void store.refresh();
  return { store, overlay, onTune, onAdd };
}

const SEED = [
  raw("a", "Repeater DB0XY", 438_650_000, { tags: ["ham", "local"] }),
  raw("b", "Radio X", 100_100_000, { mode: "wfm", bandwidth_hz: 180_000, tags: ["broadcast"] }),
  raw("c", "Tower", 118_300_000, { mode: "am", bandwidth_hz: null, notes: "approach" }),
];

afterEach(() => {
  vi.restoreAllMocks();
});

describe("bookmark api", () => {
  it("parses a bookmark and tolerates numeric ids and unknown modes", () => {
    const b = parseBookmark({ ...raw("x", "N", 1e8), id: 7, mode: "usb" });
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
    expect(onTune).toHaveBeenCalledWith({ freqHz: 438_650_000, mode: "nfm", bandwidthHz: 12_500 });
    fireEvent.click(screen.getByRole("button", { name: "Tune to Tower" }));
    expect(onTune).toHaveBeenLastCalledWith({ freqHz: 118_300_000, mode: "am" });
  });

  it("highlights the bookmark of the active frequency", async () => {
    setup(fakeServer(SEED), 100_100_000);
    expect(await screen.findByRole("button", { name: "Tune to Radio X" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
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
    expect(post?.body).toMatchObject({
      v: 1,
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
    fireEvent.click(screen.getByRole("switch", { name: "Show on spectrum" }));
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
