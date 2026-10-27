import { describe, expect, it } from "vitest";
import { parseEvent, type DecoderEvent } from "./events";
import {
  aprsSymbolName,
  describeEvent,
  formatCoords,
  isCoreEvent,
  mapLink,
  rowHeight,
} from "./render";

// The data objects are the `.expected.jsonl` lines of the golden recordings
// (crates/superhet-plugin-adapters/tests/recordings), wrapped the way the worker stores them.
const make = (kind: string, data: Record<string, unknown>, source: string): DecoderEvent => {
  const e = parseEvent({
    id: 1,
    ts_ms: 1_700_000_000_000,
    device: "dev0",
    vfo: 1,
    freq_hz: 144_800_000,
    mode: null,
    kind,
    source,
    value: String(data.summary ?? ""),
    meta: { instance: "i-1", ts: null, data },
  });
  if (!e) throw new Error("fixture");
  return e;
};

const field = (e: DecoderEvent, label: string) =>
  describeEvent(e).fields.find((f) => f.label === label)?.value;

describe("pocsag.message / flex.message", () => {
  const p = make(
    "pocsag.message",
    {
      address: 1234567,
      baud: 1200,
      function: 3,
      kind: "alpha",
      message: "Superhet POCSAG test 1",
      protocol: "pocsag",
      summary: "1234567: Superhet POCSAG test 1",
    },
    "org.superhet.decoder.pocsag",
  );

  it("shows address, function, type and message", () => {
    const v = describeEvent(p);
    expect(v.label).toBe("POCSAG");
    expect(v.headline).toBe("RIC 1234567");
    expect(v.tags).toEqual(["F3", "alpha", "1200 baud"]);
    expect(v.detail).toBe("Superhet POCSAG test 1");
    expect(field(p, "Function")).toBe("3 (alphanumeric)");
    expect(field(p, "Message")).toBe("Superhet POCSAG test 1");
    expect(v.raw).toContain('"baud": 1200');
  });

  it("handles a continuation word without an address", () => {
    const e = make(
      "pocsag.message",
      { address: null, function: null, message: "", baud: 512 },
      "x",
    );
    expect(describeEvent(e).headline).toBe("continuation");
    expect(describeEvent(e).detail).toBe("(empty)");
  });

  it("shows a FLEX capcode and group calls", () => {
    const e = make(
      "flex.message",
      {
        baud: 1600,
        capcode: 1234,
        capcodes: [1234, 1235, 1236],
        cycle: 3,
        frame: 70,
        kind: "alpha",
        levels: 2,
        message: "Hello",
        phase: "A",
        protocol: "flex",
        summary: "1234: Hello",
      },
      "org.superhet.decoder.pocsag",
    );
    const v = describeEvent(e);
    expect(v.label).toBe("FLEX");
    expect(v.headline).toBe("Capcode 1234 +2");
    expect(field(e, "Group capcodes")).toBe("1234, 1235, 1236");
    expect(field(e, "Phase")).toBe("A");
  });
});

describe("aprs.packet", () => {
  const pos = make(
    "aprs.packet",
    {
      comment: "Superhet APRS test",
      dest: "APRS",
      info: "!5230.50N/01324.30E>Superhet APRS test",
      lat: 52.50833,
      lon: 13.405,
      path: ["WIDE1-1", "WIDE2-1"],
      source: "N0CALL-9",
      summary: "N0CALL-9: 52.50833, 13.40500 Superhet APRS test",
      symbol: "/>",
      type: "position",
    },
    "org.superhet.decoder.aprs",
  );

  it("shows callsign, path, position, comment and a map link", () => {
    const v = describeEvent(pos);
    expect(v.headline).toBe("N0CALL-9");
    expect(v.tags).toEqual(["position", "via WIDE1-1,WIDE2-1"]);
    expect(v.detail).toBe("52.50833, 13.40500 · Superhet APRS test");
    expect(field(pos, "Symbol")).toBe("/> · car");
    expect(field(pos, "Via")).toBe("WIDE1-1, WIDE2-1");
    expect(v.link?.href).toBe(
      "https://www.openstreetmap.org/?mlat=52.50833&mlon=13.40500#map=14/52.50833/13.40500",
    );
  });

  it("shows messages and statuses without a link", () => {
    const m = make(
      "aprs.packet",
      {
        addressee: "N0CALL-1",
        dest: "APRS",
        info: ":N0CALL-1 :Hello from Superhet{42",
        message: "Hello from Superhet",
        message_id: "42",
        path: ["WIDE2-1"],
        source: "N0CALL-7",
        type: "message",
      },
      "org.superhet.decoder.aprs",
    );
    expect(describeEvent(m).detail).toBe("to N0CALL-1: Hello from Superhet");
    expect(describeEvent(m).link).toBeNull();
    const s = make(
      "aprs.packet",
      { source: "N0CALL", path: [], status: "Testing", type: "status", info: ">Testing" },
      "org.superhet.decoder.aprs",
    );
    expect(describeEvent(s).detail).toBe("Testing");
    expect(describeEvent(s).tags).toEqual(["status"]);
  });

  it("names objects and rejects impossible positions", () => {
    const o = make(
      "aprs.packet",
      { source: "N0CALL", name: "EVENT", type: "object", lat: 91, lon: 0, path: [] },
      "org.superhet.decoder.aprs",
    );
    expect(describeEvent(o).headline).toBe("EVENT (N0CALL)");
    expect(describeEvent(o).link).toBeNull();
    expect(formatCoords(91, 0)).toBeNull();
    expect(mapLink(0, 181)).toBeNull();
    expect(aprsSymbolName("/_")).toBe("weather station");
    expect(aprsSymbolName("Zz")).toBeNull();
  });
});

describe("rtl433.reading", () => {
  const r = make(
    "rtl433.reading",
    {
      battery_ok: 1,
      channel: 2,
      humidity: 48,
      id: 165,
      model: "Nexus-TH",
      summary: "Nexus-TH id=165 ...",
      temperature_C: 21.5,
    },
    "org.superhet.decoder.rtl433",
  );

  it("shows model, id and the key readings", () => {
    const v = describeEvent(r);
    expect(v.headline).toBe("Nexus-TH #165");
    expect(v.tags).toEqual(["ch 2"]);
    expect(v.detail).toBe("21.5 °C · 48 % · battery ok");
    expect(field(r, "Temperature")).toBe("21.5 °C");
    expect(field(r, "Battery")).toBe("ok");
  });

  it("reports a low battery and keeps unknown scalar fields", () => {
    const e = make(
      "rtl433.reading",
      { model: "Acurite-Tower", id: "A1", battery_ok: 0, temperature_F: 70.1234, button: 1 },
      "org.superhet.decoder.rtl433",
    );
    expect(describeEvent(e).detail).toBe("70.12 °F · battery low");
    expect(field(e, "button")).toBe("1");
  });
});

describe("generic fallback", () => {
  it("shows an unknown kind with its data as fields", () => {
    const e = make(
      "echo.block",
      { bytes: 960, seq: 4, nested: { a: 1 }, list: ["a", "b"] },
      "org.superhet.example.echo",
    );
    const v = describeEvent(e);
    expect(v.label).toBe("echo.block");
    expect(field(e, "bytes")).toBe("960");
    expect(field(e, "list")).toBe("a, b");
    expect(v.fields.some((f) => f.label === "nested")).toBe(false);
    expect(v.raw).toContain('"nested"');
  });

  it("copes with events that have no data", () => {
    const e = parseEvent({
      id: 2,
      ts_ms: 1,
      device: "-",
      vfo: null,
      kind: "weird",
      source: "p",
      value: "v",
      meta: {},
    });
    expect(e && describeEvent(e).raw).toBeNull();
    expect(e && describeEvent(e).headline).toBe("v");
  });
});

describe("row height", () => {
  it("is compact for the built-in detectors only", () => {
    const core = parseEvent({ id: 3, ts_ms: 1, device: "d", vfo: 1, kind: "dtmf", value: "1" });
    const plugin = parseEvent({
      id: 4,
      ts_ms: 1,
      device: "d",
      vfo: 1,
      kind: "dtmf",
      source: "p",
      value: "1",
    });
    expect(core && isCoreEvent(core)).toBe(true);
    expect(core && rowHeight(core)).toBe(32);
    // A plugin may reuse a core kind name; the source decides.
    expect(plugin && rowHeight(plugin)).toBe(52);
  });
});
