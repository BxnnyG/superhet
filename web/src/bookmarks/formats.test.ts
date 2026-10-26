import { describe, expect, it } from "vitest";
import {
  exportChirp,
  exportGqrx,
  exportSuperhet,
  parseChirp,
  parseCsvLine,
  parseGqrx,
  parseImport,
  parseSdrpp,
  parseSuperhet,
} from "./formats";
import { newBookmarks, newInput, parseTags, type BookmarkInput } from "./model";

const GQRX = `# Tag name          ; color
Untagged            ;  #c0c0c0
Marine              ;  #00aaff
Amateur             ;  #ff0000

# Frequency ; Name ; Modulation ; Bandwidth ; Tags
  145800000 ; ISS          ; Narrow FM ; 12500 ; Amateur
  161975000 ; AIS 1        ; Narrow FM ; 25000 ; Marine,Ships
  100000000 ; Radio X      ; WFM (stereo) ; 160000 ; Untagged
  14200000  ; 20 m SSB     ; Upper Side Band ; 2400 ; Amateur
  0         ; Broken       ; AM ; 10000 ; Untagged
  121500000 ; Guard        ; AM ; 8000 ;
`;

const SDRPP = JSON.stringify({
  bookmarkDisplayMode: 1,
  lists: {
    General: {
      showOnWaterfall: true,
      bookmarks: {
        "Radio X": { bandwidth: 150000.0, frequency: 100000000.0, mode: 1 },
        ISS: { bandwidth: 12500, frequency: 145800000, mode: 0 },
      },
    },
    Ham: {
      bookmarks: {
        "20m": { frequency: 14200000, bandwidth: 2700, mode: 4 },
        Broken: { bandwidth: 1, mode: 0 },
      },
    },
  },
  selectedList: "General",
});

describe("gqrx CSV", () => {
  const r = parseGqrx(GQRX);

  it("reads frequency, name, mode, bandwidth and tags", () => {
    expect(r.format).toBe("gqrx");
    expect(r.bookmarks).toHaveLength(5);
    expect(r.bookmarks[0]).toMatchObject({
      name: "ISS",
      freqHz: 145_800_000,
      mode: "nfm",
      bandwidthHz: 12_500,
      tags: ["Amateur"],
      color: "#ff0000",
    });
    expect(r.bookmarks[1]?.tags).toEqual(["Marine", "Ships"]);
    expect(r.bookmarks[1]?.color).toBe("#00aaff");
    expect(r.bookmarks[2]).toMatchObject({ mode: "wfm", tags: [], color: null });
    expect(r.bookmarks[4]).toMatchObject({ name: "Guard", mode: "am", tags: [] });
  });

  it("keeps unsupported modulations in the notes and counts bad rows", () => {
    expect(r.bookmarks[3]).toMatchObject({ mode: null, notes: "Mode: Upper Side Band" });
    expect(r.unsupportedMode).toBe(1);
    expect(r.skipped).toBe(1);
  });

  it("round-trips through the exporter", () => {
    const text = exportGqrx(
      r.bookmarks.map((b) => ({ ...b, id: "x", createdMs: 0, updatedMs: 0 })),
    );
    const again = parseGqrx(text);
    expect(again.bookmarks).toHaveLength(r.bookmarks.length);
    expect(again.bookmarks[0]).toMatchObject({ name: "ISS", freqHz: 145_800_000, mode: "nfm" });
    expect(again.bookmarks[1]?.tags).toEqual(["Marine", "Ships"]);
  });
});

describe("SDR++ frequency manager", () => {
  const r = parseSdrpp(JSON.parse(SDRPP));

  it("maps the mode ids and uses list names as tags", () => {
    expect(r.format).toBe("sdrpp");
    expect(r.bookmarks).toHaveLength(3);
    const byName = Object.fromEntries(r.bookmarks.map((b) => [b.name, b]));
    expect(byName["Radio X"]).toMatchObject({
      freqHz: 100_000_000,
      mode: "wfm",
      bandwidthHz: 150_000,
      tags: [],
    });
    expect(byName.ISS).toMatchObject({ mode: "nfm", tags: [] });
    expect(byName["20m"]).toMatchObject({ mode: "usb", tags: ["Ham"] });
    expect(r.skipped).toBe(1);
    expect(r.unsupportedMode).toBe(0);
  });

  it("accepts a single exported list", () => {
    const one = parseImport(JSON.stringify({ bookmarks: { A: { frequency: 1e8, mode: 2 } } }));
    expect(one.format).toBe("sdrpp");
    expect(one.bookmarks[0]).toMatchObject({ name: "A", mode: "am", bandwidthHz: null });
  });
});

describe("Superhet JSON", () => {
  const sample: BookmarkInput[] = [
    newInput({
      name: "Home",
      freqHz: 433_500_000,
      mode: "nfm",
      bandwidthHz: 12_500,
      tags: ["ham", "local"],
      color: "#fbbf24",
      notes: "calling",
    }),
    newInput({ name: "FM", freqHz: 100_100_000 }),
  ];

  it("round-trips and ignores server ids", () => {
    const text = exportSuperhet(sample.map((b) => ({ ...b })));
    expect(JSON.parse(text)).toMatchObject({ format: "superhet-bookmarks", v: 2 });
    const r = parseImport(text);
    expect(r.format).toBe("superhet");
    expect(r.bookmarks).toEqual(sample);
  });

  it("reads the API list shape and drops invalid entries", () => {
    const r = parseSuperhet({
      v: 1,
      bookmarks: [
        { id: 7, name: "A", freq_hz: 1e8, mode: "xyz", tags: ["a", "A", " "], created_ms: 1 },
        { name: "no freq" },
        "junk",
      ],
    });
    expect(r.bookmarks).toHaveLength(1);
    expect(r.bookmarks[0]).toMatchObject({ mode: null, tags: ["a"] });
    expect(r.skipped).toBe(2);
  });
});

describe("format detection", () => {
  it("rejects empty and unknown files", () => {
    expect(() => parseImport("  ")).toThrow(/empty/);
    expect(() => parseImport("hello")).toThrow(/Unrecognised/);
    expect(() => parseImport("{nope")).toThrow(/JSON/);
    expect(() => parseImport('{"x":1}')).toThrow(/No bookmarks/);
  });

  it("detects each format", () => {
    expect(parseImport(GQRX).format).toBe("gqrx");
    expect(parseImport(SDRPP).format).toBe("sdrpp");
    expect(parseImport('{"v":1,"bookmarks":[]}').format).toBe("superhet");
  });
});

describe("helpers", () => {
  it("parses tags", () => {
    expect(parseTags("a, b ,,A;c")).toEqual(["a", "b", "c"]);
  });

  it("merges by frequency and name", () => {
    const existing = [{ name: "ISS", freqHz: 145_800_000 }];
    const incoming = parseGqrx(GQRX).bookmarks;
    const fresh = newBookmarks(existing, incoming);
    expect(fresh).toHaveLength(incoming.length - 1);
    expect(fresh.some((b) => b.name === "ISS")).toBe(false);
  });
});

describe("Superhet JSON v2", () => {
  const full: BookmarkInput = newInput({
    name: "Dispatch",
    freqHz: 392_012_500,
    mode: "nfm",
    bandwidthHz: 25_000,
    squelchDb: -75,
    squelchAuto: true,
    stepHz: 25_000,
    toneMode: "dcs",
    dcsCode: 23,
    dcsPolarity: "R",
    digital: {
      protocol: "tetra",
      timeslot: 3,
      talkgroup: 262_001,
      colourCode: null,
      mcc: 262,
      mnc: 1,
      decoderPluginId: "tetra-kit",
    },
    groupId: "g1",
    favourite: true,
    scan: { include: true, priority: 2, dwellMs: 3000 },
  });

  it("round-trips every field and carries groups by name", () => {
    const text = exportSuperhet([full], [{ id: "g1", name: "TETRA", position: 0 }]);
    const parsed = JSON.parse(text) as { groups: unknown; bookmarks: Record<string, unknown>[] };
    expect(parsed.groups).toEqual([{ name: "TETRA" }]);
    expect(parsed.bookmarks[0]).toMatchObject({ group: "TETRA", dcs_code: 23, tone_mode: "dcs" });
    expect(parsed.bookmarks[0]).not.toHaveProperty("group_id");
    const r = parseImport(text);
    // The group id is the server's; the import carries the name instead.
    expect(r.bookmarks).toEqual([{ ...full, groupId: null, groupName: "TETRA" }]);
  });
});

describe("CHIRP CSV", () => {
  const CHIRP = `Location,Name,Frequency,Duplex,Offset,Tone,rToneFreq,cToneFreq,DtcsCode,DtcsPolarity,RxDtcsCode,CrossMode,Mode,TStep,Skip,Power,Comment,URCALL,RPT1CALL,RPT2CALL,DVCODE
0,DB0XY,438.650000,-,7.600000,TSQL,88.5,88.5,023,NN,023,Tone->Tone,FM,12.50,,5.0W,"Local, repeater",,,,
1,Digi,145.500000,,0.600000,DTCS,88.5,88.5,047,NR,047,Tone->Tone,NFM,5.00,S,5.0W,,,,,
2,Plain,446.006250,,0.600000,,88.5,88.5,023,NN,023,Tone->Tone,DV,6.25,,5.0W,,,,,
3,Bad,x,,,,,,,,,,FM,,,,,,,,
`;

  it("imports tones, step, scan skip and comment", () => {
    const r = parseImport(CHIRP);
    expect(r.format).toBe("chirp");
    expect(r.bookmarks).toHaveLength(3);
    expect(r.skipped).toBe(1);
    expect(r.unsupportedMode).toBe(1);
    expect(r.bookmarks[0]).toMatchObject({
      name: "DB0XY",
      freqHz: 438_650_000,
      mode: "nfm",
      toneMode: "ctcss",
      ctcssHz: 88.5,
      stepHz: 12_500,
      notes: "Local, repeater",
      scan: { include: true },
    });
    expect(r.bookmarks[1]).toMatchObject({
      toneMode: "dcs",
      dcsCode: 47,
      dcsPolarity: "R",
      scan: { include: false },
    });
    expect(r.bookmarks[2]).toMatchObject({ toneMode: null, mode: null, stepHz: 6_250 });
  });

  it("exports a file it can read back", () => {
    const back = parseChirp(exportChirp(parseChirp(CHIRP).bookmarks));
    expect(back.bookmarks).toHaveLength(3);
    expect(back.bookmarks[0]).toMatchObject({ toneMode: "ctcss", ctcssHz: 88.5, stepHz: 12_500 });
    expect(back.bookmarks[1]).toMatchObject({ toneMode: "dcs", dcsCode: 47, dcsPolarity: "R" });
  });

  it("splits quoted CSV cells", () => {
    expect(parseCsvLine('a,"b,c","d ""e""",')).toEqual(["a", "b,c", 'd "e"', ""]);
  });
});
