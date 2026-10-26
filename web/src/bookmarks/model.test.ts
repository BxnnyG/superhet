import { describe, expect, it } from "vitest";
import {
  bookmarkTarget,
  digitalBadge,
  filterBookmarks,
  formatDcs,
  groupSections,
  isFiltering,
  NO_FILTER,
  nearBookmarks,
  newInput,
  receiverState,
  sortBy,
  toneBadge,
  type Bookmark,
  type BookmarkGroup,
} from "./model";

const bm = (id: string, name: string, freqHz: number, over: Partial<Bookmark> = {}): Bookmark => ({
  ...newInput({ name, freqHz }),
  id,
  createdMs: 1,
  updatedMs: 1,
  ...over,
});

const GROUPS: BookmarkGroup[] = [
  { id: "g2", name: "Ham", position: 1 },
  { id: "g1", name: "Air", position: 0 },
];

describe("badges", () => {
  it("formats CTCSS, DCS and digital channels", () => {
    expect(toneBadge(bm("a", "A", 1, { toneMode: "ctcss", ctcssHz: 88.5 }))).toBe("88.5");
    expect(toneBadge(bm("a", "A", 1, { toneMode: "dcs", dcsCode: 23, dcsPolarity: "R" }))).toBe(
      "DCS 023R",
    );
    expect(toneBadge(bm("a", "A", 1, { toneMode: "none" }))).toBeNull();
    expect(toneBadge(bm("a", "A", 1))).toBeNull();
    expect(formatDcs(754, null)).toBe("754N");
    const tetra = bm("a", "A", 1, {
      digital: {
        protocol: "tetra",
        timeslot: 2,
        talkgroup: 9,
        colourCode: null,
        mcc: null,
        mnc: null,
        decoderPluginId: null,
      },
    });
    expect(digitalBadge(tetra)).toBe("TETRA TS2 TG 9");
    expect(digitalBadge(bm("a", "A", 1))).toBeNull();
  });
});

describe("bookmarkTarget", () => {
  it("leaves unset fields out and always states the sub-tone", () => {
    expect(bookmarkTarget(bm("a", "A", 100e6))).toEqual({ freqHz: 100e6, toneSquelchHz: null });
    expect(
      bookmarkTarget(
        bm("a", "A", 145.5e6, {
          mode: "nfm",
          bandwidthHz: 12_500,
          squelchDb: -75,
          stepHz: 12_500,
          toneMode: "ctcss",
          ctcssHz: 123,
        }),
      ),
    ).toEqual({
      freqHz: 145.5e6,
      mode: "nfm",
      bandwidthHz: 12_500,
      squelchDbfs: -75,
      stepHz: 12_500,
      toneSquelchHz: 123,
    });
  });

  it("does not apply DCS (stored only) or a non-standard CTCSS tone", () => {
    expect(
      bookmarkTarget(bm("a", "A", 1, { toneMode: "dcs", dcsCode: 23, dcsPolarity: "N" })),
    ).toMatchObject({ toneSquelchHz: null });
    expect(bookmarkTarget(bm("a", "A", 1, { toneMode: "ctcss", ctcssHz: 90 }))).toMatchObject({
      toneSquelchHz: null,
    });
  });

  it("marks an automatic squelch", () => {
    expect(bookmarkTarget(bm("a", "A", 1, { squelchAuto: true, squelchDb: -70 }))).toMatchObject({
      squelchAuto: true,
      squelchDbfs: -70,
    });
  });
});

describe("filter, sort, group, near", () => {
  const list = [
    bm("1", "Tower", 118.3e6, { groupId: "g1", favourite: true, updatedMs: 5 }),
    bm("2", "Repeater", 438.65e6, {
      groupId: "g2",
      tags: ["ham"],
      toneMode: "ctcss",
      ctcssHz: 88.5,
    }),
    bm("3", "Loose", 100e6, { updatedMs: 9 }),
    bm("4", "ATIS", 118.7e6, { groupId: "gone" }),
  ];

  it("filters by group, favourite and text including the tone", () => {
    expect(filterBookmarks(list, "", "", { group: "g1" }).map((b) => b.id)).toEqual(["1"]);
    expect(filterBookmarks(list, "", "", { group: "none" }).map((b) => b.id)).toEqual(["3"]);
    expect(filterBookmarks(list, "", "", { favouritesOnly: true }).map((b) => b.id)).toEqual(["1"]);
    expect(filterBookmarks(list, "88.5", "").map((b) => b.id)).toEqual(["2"]);
    expect(isFiltering(NO_FILTER)).toBe(false);
    expect(isFiltering({ ...NO_FILTER, favouritesOnly: true })).toBe(true);
  });

  it("sorts by frequency, name and recent change", () => {
    expect(sortBy(list, "freq").map((b) => b.id)).toEqual(["3", "1", "4", "2"]);
    expect(sortBy(list, "name").map((b) => b.id)).toEqual(["4", "3", "2", "1"]);
    expect(sortBy(list, "recent").map((b) => b.id)[0]).toBe("3");
  });

  it("sections follow the group order; unknown groups count as ungrouped", () => {
    const sections = groupSections(sortBy(list, "freq"), GROUPS, false);
    expect(sections.map((s) => [s.group?.name ?? null, s.items.map((b) => b.id)])).toEqual([
      ["Air", ["1"]],
      ["Ham", ["2"]],
      [null, ["3", "4"]],
    ]);
    const filtered = groupSections([list[0] as Bookmark], GROUPS, true);
    expect(filtered.map((s) => s.group?.name)).toEqual(["Air"]);
    expect(groupSections([], GROUPS, false).map((s) => s.items.length)).toEqual([0, 0]);
  });

  it("finds the bookmarks near a frequency, nearest first", () => {
    expect(nearBookmarks(list, 118_500_000).map((b) => b.id)).toEqual(["1", "4"]);
    expect(nearBookmarks(list, 118_500_000, 100_000)).toEqual([]);
    expect(nearBookmarks(list, null)).toEqual([]);
    expect(nearBookmarks(list, 118_500_000, 1e9, 1)).toHaveLength(1);
  });
});

describe("receiverState", () => {
  it("captures squelch, step and the tone squelch of a VFO", () => {
    const vfo = {
      id: 1,
      freqHz: 1,
      mode: "nfm" as const,
      bandwidthHz: 12_500,
      squelchDbfs: -80,
      deemphasis: null,
      toneSquelchHz: 88.5,
      offsetHz: 0,
      status: "active" as const,
    };
    expect(receiverState(vfo, true, 25_000)).toEqual({
      squelchDb: -80,
      squelchAuto: true,
      stepHz: 25_000,
      toneMode: "ctcss",
      ctcssHz: 88.5,
    });
    expect(receiverState(null, false, "auto")).toEqual({
      squelchDb: null,
      squelchAuto: false,
      stepHz: null,
    });
  });
});
