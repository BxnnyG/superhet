import { describe, expect, it, vi } from "vitest";
import { newInput, type Bookmark } from "../bookmarks/model";
import type { Vfo } from "../vfo/api";
import { builtinPlans } from "../channels/catalog";
import { buildSections, flatten, type PaletteDeps } from "./items";

const vfo = (id: number, patch: Partial<Vfo> = {}): Vfo => ({
  id,
  freqHz: 100_000_000 + id * 1_000_000,
  mode: "wfm",
  bandwidthHz: 180_000,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
  ...patch,
});

const bookmark = (id: string, patch: Partial<Bookmark> = {}): Bookmark => ({
  ...newInput({ name: `Bookmark ${id}`, freqHz: 433_920_000 }),
  id,
  createdMs: 0,
  updatedMs: 0,
  ...patch,
});

function deps(patch: Partial<PaletteDeps> = {}) {
  const calls = {
    tune: vi.fn(),
    remember: vi.fn(),
    act: vi.fn(),
    selectVfo: vi.fn(),
    showTab: vi.fn(),
    toggleAudio: vi.fn(),
    exportLog: vi.fn(),
    openSettings: vi.fn(),
  };
  const value: PaletteDeps = {
    vfos: [vfo(1), vfo(2, { mode: "nfm", freqHz: 145_500_000 })],
    selectedId: 1,
    bookmarks: [
      bookmark("a", {
        name: "Munich Tower",
        freqHz: 118_100_000,
        mode: "am",
        tags: ["airband"],
      }),
      bookmark("b", {
        name: "PMR channel 1",
        freqHz: 446_006_250,
        mode: "nfm",
        toneMode: "ctcss",
        ctcssHz: 88.5,
        tags: ["pmr"],
        favourite: true,
      }),
    ],
    recent: [{ freqHz: 145_500_000, mode: "nfm" }],
    audioEnabled: false,
    ...calls,
    ...patch,
  };
  return { value, calls };
}

const groups = (d: PaletteDeps, q: string) => buildSections(d, q).map((s) => s.group);
const titles = (d: PaletteDeps, q: string) => flatten(buildSections(d, q)).map((i) => i.title);

describe("buildSections", () => {
  it("offers recent, favourites and the actions without a query", () => {
    const { value } = deps();
    expect(groups(value, "")).toEqual(["Bookmarks", "Recent", "Actions"]);
    const sections = buildSections(value, "");
    expect(sections[0]?.items.map((i) => i.title)).toEqual(["PMR channel 1"]);
    expect(titles(value, "")).toContain("New VFO at the display centre");
  });

  it("puts a Tune entry first for a frequency command", () => {
    const { value, calls } = deps();
    const first = flatten(buildSections(value, "446.00625 nfm ctcss 88.5"))[0];
    expect(first?.group).toBe("Tune");
    expect(first?.title).toBe("Tune to 446.006 250 MHz · NFM · CTCSS 88.5");
    first?.run({ newVfo: false });
    expect(calls.tune).toHaveBeenCalledWith(
      { freqHz: 446_006_250, mode: "nfm", toneSquelchHz: 88.5 },
      false,
    );
    expect(calls.remember).toHaveBeenCalledWith({ freqHz: 446_006_250, mode: "nfm", toneHz: 88.5 });
    first?.run({ newVfo: true });
    expect(calls.tune).toHaveBeenLastCalledWith(expect.anything(), true);
  });

  it("finds bookmarks by name, tag and frequency and tunes with the full state", () => {
    const { value, calls } = deps();
    expect(titles(value, "tower")[0]).toBe("Munich Tower");
    expect(titles(value, "airband")[0]).toBe("Munich Tower");
    expect(titles(value, "446.006").find((t) => t.startsWith("PMR"))).toBe("PMR channel 1");
    const pmr = flatten(buildSections(value, "pmr")).find((i) => i.id === "bookmark:b");
    pmr?.run({ newVfo: false });
    expect(calls.tune).toHaveBeenCalledWith(
      expect.objectContaining({ freqHz: 446_006_250, mode: "nfm", toneSquelchHz: 88.5 }),
      false,
    );
    expect(pmr?.subtitle).toContain("CTCSS 88.5");
  });

  it("finds actions by title and keyword and shows their shortcut", () => {
    const { value, calls } = deps();
    const mute = flatten(buildSections(value, "mute")).find((i) => i.id === "action:mute");
    expect(mute?.hint).toEqual(["M"]);
    mute?.run({ newVfo: false });
    expect(calls.act).toHaveBeenCalledWith({ type: "listen" });
    const tab = flatten(buildSections(value, "decoders")).find(
      (i) => i.id === "action:tab-decoders",
    );
    tab?.run({ newVfo: false });
    expect(calls.showTab).toHaveBeenCalledWith("decoders");
    const log = flatten(buildSections(value, "export log"))[0];
    expect(log?.id).toBe("action:export-log");
    log?.run({ newVfo: false });
    expect(calls.exportLog).toHaveBeenCalled();
  });

  it("offers mode switches except the current mode, with their number key", () => {
    const { value } = deps();
    const items = flatten(buildSections(value, "switch mode"));
    expect(items.find((i) => i.id === "action:mode-wfm")).toBeUndefined();
    expect(items.find((i) => i.id === "action:mode-nfm")?.hint).toEqual(["2"]);
  });

  it("offers VFO actions only with an active VFO", () => {
    const { value } = deps({ vfos: [], selectedId: null });
    expect(titles(value, "close")).not.toContain("Close the active VFO");
    expect(titles(value, "new vfo")).toContain("New VFO at the display centre");
  });

  it("lists the VFOs with their Alt+digit hint when searching", () => {
    const { value, calls } = deps();
    const second = flatten(buildSections(value, "vfo 2")).find((i) => i.id === "vfo:2");
    expect(second?.hint).toEqual(["Alt", "2"]);
    second?.run({ newVfo: false });
    expect(calls.selectVfo).toHaveBeenCalledWith(2);
  });

  it("names the audio toggle after the current state", () => {
    expect(titles(deps({ audioEnabled: true }).value, "audio")).toContain("Turn audio off");
    expect(titles(deps({ audioEnabled: false }).value, "audio")).toContain("Turn audio on");
  });

  it("returns nothing for a query that matches nothing", () => {
    const { value } = deps();
    expect(buildSections(value, "qqqqzzzz")).toEqual([]);
  });

  it("ranks data above actions for the same text", () => {
    const { value } = deps();
    expect(groups(value, "tower")[0]).toBe("Bookmarks");
  });
});

describe("channel words", () => {
  const withChannels = () => {
    const tuneChannel = vi.fn();
    const { value } = deps({
      channels: { plans: builtinPlans(), region: "DE", tuneChannel },
    });
    return { value, tuneChannel };
  };

  it("resolves 'pmr 3', 'dab 5c', 'marine 16' and 'ch16' through the plans", () => {
    const { value } = withChannels();
    expect(titles(value, "pmr 3")[0]).toBe("PMR 3 · 446.031 250 MHz");
    expect(groups(value, "pmr 3")[0]).toBe("Channels");
    expect(titles(value, "freenet 2")[0]).toMatch(/^Freenet 2 · 149\.037 500/);
    expect(titles(value, "dab 5c")[0]).toMatch(/^DAB 5C · 178\.352/);
    expect(titles(value, "marine 16")[0]).toMatch(/^Marine 16 · International distress/);
    expect(titles(value, "ch16")[0]).toMatch(/^Marine 16/);
  });

  it("runs the tune callback, Alt+Enter asks for a new VFO, DAB never does", () => {
    const { value, tuneChannel } = withChannels();
    const [pmr] = flatten(buildSections(value, "pmr 3"));
    pmr?.run({ newVfo: false });
    pmr?.run({ newVfo: true });
    expect(tuneChannel).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({ id: "pmr446" }),
      expect.objectContaining({ number: "3" }),
      false,
    );
    expect(tuneChannel.mock.calls[1]?.[2]).toBe(true);
    const [dab] = flatten(buildSections(value, "dab 5c"));
    expect(pmr?.newVfoAlt).toBe(true);
    expect(dab?.newVfoAlt).toBe(false);
    expect(dab?.subtitle).toMatch(/device centre/);
  });

  it("adds nothing without plans, for a plain frequency or an empty query", () => {
    const { value } = deps();
    expect(groups(value, "pmr 3")).not.toContain("Channels");
    const { value: v2 } = withChannels();
    expect(groups(v2, "446.00625")).not.toContain("Channels");
    expect(groups(v2, "")).not.toContain("Channels");
  });
});
