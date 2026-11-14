import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { describeEvent } from "../decoders/render";
import { AisMapView } from "./AisMapView";
import {
  AIS_VESSEL,
  DROP_AFTER_MS,
  MAX_TRACK,
  STALE_AFTER_MS,
  displayName,
  formatSpeed,
  groupOf,
  heardAgo,
  matches,
  pointing,
  positioned,
  vesselOpacity,
  vessels,
} from "./model";

// Leaflet needs a real layout; the map has its own code. The (mocked) map loads nothing.
const mapProps = vi.fn();
vi.mock("./AisMap", () => ({
  AisMap: (props: { vessels: { mmsi: number }[]; selected: number | null }) => {
    mapProps(props);
    return <div data-testid="map">{props.vessels.map((v) => v.mmsi).join(",")}</div>;
  },
}));

const NOW = 1_790_000_000_000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
  vi.setSystemTime(NOW);
  mapProps.mockClear();
});
afterEach(() => vi.useRealTimers());

let nextId = 1;
const report = (agoMs: number, data: Record<string, unknown>): DecoderEvent => {
  const e = parseEvent({
    id: nextId++,
    ts_ms: NOW - agoMs,
    device: "dev0",
    vfo: 1,
    freq_hz: 162_000_000,
    mode: null,
    kind: AIS_VESSEL,
    source: "org.superhet.decoder.ais",
    value: "",
    meta: { instance: "i-1", data: { class: "A", summary: "x", ...data } },
  });
  if (!e) throw new Error("fixture");
  return e;
};

const flush = () => act(() => vi.advanceTimersByTimeAsync(0));

describe("vessels", () => {
  it("keeps the newest picture and the track, whatever the event order", () => {
    const events = [
      report(1000, { mmsi: 244670316, lat: 52.1, lon: 4.2, sog_kn: 12.3, cog_deg: 270 }),
      report(3000, { mmsi: 244670316, lat: 52.0, lon: 4.1, name: "EVER GIVEN", ship_type: 70 }),
      report(2000, { mmsi: 244670316, lat: 52.0, lon: 4.1 }),
    ];
    const [v, ...rest] = vessels(events, NOW);
    expect(rest).toEqual([]);
    expect(v?.name).toBe("EVER GIVEN");
    expect([v?.lat, v?.lon, v?.sogKn, v?.cogDeg]).toEqual([52.1, 4.2, 12.3, 270]);
    expect(v?.reports).toBe(3);
    // The repeated position adds no track point.
    expect(v?.track.map((p) => [p.lat, p.lon])).toEqual([
      [52.0, 4.1],
      [52.1, 4.2],
    ]);
  });

  it("lists vessels without a position but does not draw them", () => {
    const list = vessels([report(1000, { mmsi: 211000001, name: "NO FIX" })], NOW);
    expect(list).toHaveLength(1);
    expect(positioned(list)).toEqual([]);
  });

  it("ignores broken events and other kinds", () => {
    const events = [
      report(1000, { mmsi: 0, lat: 1, lon: 1 }),
      report(1000, { lat: 1, lon: 1 }),
      report(1000, { mmsi: 211000002, lat: 95, lon: 1 }),
    ];
    const list = vessels(events, NOW);
    expect(list.map((v) => v.mmsi)).toEqual([211000002]);
    expect(positioned(list)).toEqual([]);
  });

  it("caps the track and drops vessels that were not heard for hours", () => {
    const events = Array.from({ length: MAX_TRACK + 10 }, (_, i) =>
      report(60_000 + (MAX_TRACK + 10 - i) * 1000, { mmsi: 1, lat: 50 + i / 1000, lon: 8 }),
    );
    expect(vessels(events, NOW)[0]?.track).toHaveLength(MAX_TRACK);
    expect(vessels([report(DROP_AFTER_MS + 1, { mmsi: 2, lat: 1, lon: 1 })], NOW)).toEqual([]);
  });

  it("keeps what an older event knew when the newest does not", () => {
    const [v] = vessels(
      [
        report(5000, { mmsi: 3, name: "OLD NAME", destination: "HAMBURG", lat: 1, lon: 1 }),
        report(1000, { mmsi: 3, lat: 1.1, lon: 1.1 }),
      ],
      NOW,
    );
    expect([v?.name, v?.destination]).toEqual(["OLD NAME", "HAMBURG"]);
  });
});

describe("symbols and text", () => {
  it("groups by ship type and station class", () => {
    const g = (cls: "A" | "B" | "base" | "aton" | "sar", shipType: number | null) =>
      groupOf({ cls, shipType });
    expect(g("A", 70)).toBe("cargo");
    expect(g("A", 84)).toBe("tanker");
    expect(g("A", 69)).toBe("passenger");
    expect(g("B", 30)).toBe("fishing");
    expect(g("B", 36)).toBe("sailing");
    expect(g("B", 37)).toBe("pleasure");
    expect(g("A", 52)).toBe("tug");
    expect(g("A", 51)).toBe("special");
    expect(g("A", 45)).toBe("highspeed");
    expect(g("A", null)).toBe("other");
    expect(g("base", null)).toBe("base");
    expect(g("aton", 14)).toBe("aton");
    expect(g("sar", null)).toBe("sar");
  });

  it("points to the heading, else the course, and is a dot at rest", () => {
    expect(pointing({ headingDeg: 90, cogDeg: 80, sogKn: 5 })).toBe(90);
    expect(pointing({ headingDeg: null, cogDeg: 80, sogKn: 5 })).toBe(80);
    expect(pointing({ headingDeg: null, cogDeg: 80, sogKn: 0.1 })).toBeNull();
    expect(pointing({ headingDeg: 90, cogDeg: null, sogKn: 0 })).toBe(90);
  });

  it("writes speeds in knots and km/h", () => {
    expect(formatSpeed(12.3)).toBe("12.3 kn · 22.8 km/h");
    expect(formatSpeed(null)).toBeNull();
  });

  it("names a vessel by name, callsign or MMSI and searches all of them", () => {
    expect(displayName({ name: "ANNA", callsign: "DA1", mmsi: 1 })).toBe("ANNA");
    expect(displayName({ name: null, callsign: "DA1", mmsi: 1 })).toBe("DA1");
    expect(displayName({ name: null, callsign: null, mmsi: 211000001 })).toBe("211000001");
    const [v] = vessels(
      [report(0, { mmsi: 211000001, name: "ANNA", callsign: "DA1", destination: "KIEL", imo: 9 })],
      NOW,
    );
    if (v === undefined) throw new Error("fixture");
    for (const q of ["anna", "211000", "da1", "kiel", "9", ""]) expect(matches(v, q)).toBe(true);
    expect(matches(v, "hamburg")).toBe(false);
  });

  it("fades stale vessels and says when they were heard", () => {
    expect(vesselOpacity({ lastHeardMs: NOW - 1000 }, NOW)).toBe(1);
    expect(vesselOpacity({ lastHeardMs: NOW - STALE_AFTER_MS - 1 }, NOW)).toBe(0.45);
    expect(heardAgo({ lastHeardMs: NOW - 5000 }, NOW)).toBe("just now");
    expect(heardAgo({ lastHeardMs: NOW - 120_000 }, NOW)).toBe("2 min ago");
  });
});

describe("the log row", () => {
  it("describes a report with name, type, speed in both units and destination", () => {
    const e = report(0, {
      mmsi: 244670316,
      name: "EVER GIVEN",
      ship_type_text: "Cargo",
      sog_kn: 12.3,
      cog_deg: 270.4,
      destination: "NLRTM",
      lat: 52.0123,
      lon: 4.1234,
      status_text: "Under way using engine",
    });
    const v = describeEvent(e);
    expect(v.label).toBe("AIS");
    expect(v.headline).toBe("EVER GIVEN");
    expect(v.tags).toEqual(["244670316", "Cargo"]);
    expect(v.detail).toBe("12.3 kn · 22.8 km/h 270° → NLRTM");
    expect(v.fields.find((f) => f.label === "Position")?.value).toBe("52.01230, 4.12340");
    expect(v.link?.href).toContain("openstreetmap.org");
  });

  it("labels aids to navigation", () => {
    const v = describeEvent(
      report(0, {
        mmsi: 992111000,
        class: "aton",
        name: "FAIRWAY 3",
        aton_type_text: "Port hand mark",
      }),
    );
    expect(v.label).toBe("AtoN");
    expect(v.tags).toContain("Port hand mark");
  });
});

describe("AisMapView", () => {
  it("lists vessels newest first and draws the ones with a position", async () => {
    render(
      <AisMapView
        events={[
          report(120_000, { mmsi: 211000001, name: "OLD", lat: 54, lon: 8, sog_kn: 3 }),
          report(5_000, { mmsi: 211000002, name: "NEW", lat: 54.1, lon: 8.1, sog_kn: 10.2 }),
          report(1_000, { mmsi: 211000003, name: "NO FIX" }),
        ]}
      />,
    );
    await flush();
    const list = within(screen.getByRole("region", { name: "AIS vessels" }));
    expect(list.getAllByRole("button").map((b) => b.textContent)).toEqual([
      "NO FIX",
      "NEW10.2 kn",
      "OLD3.0 kn",
    ]);
    expect(screen.getByTestId("map")).toHaveTextContent("211000003,211000002,211000001");
  });

  it("shows the details of the picked vessel in knots and km/h and hides them again", async () => {
    render(
      <AisMapView
        events={[
          report(10_000, {
            mmsi: 244670316,
            name: "EVER GIVEN",
            callsign: "PH1234",
            imo: 9321483,
            ship_type_text: "Cargo",
            status_text: "Under way using engine",
            lat: 52.0123,
            lon: 4.1234,
            sog_kn: 12.3,
            cog_deg: 270.4,
            heading_deg: 268,
            destination: "NLRTM",
            eta: "11-24 18:30",
            draught_m: 14.5,
            length_m: 400,
            beam_m: 58,
          }),
        ]}
      />,
    );
    await flush();
    expect(screen.queryByTestId("ais-details")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /EVER GIVEN/ }));
    const d = screen.getByTestId("ais-details");
    expect(d).toHaveTextContent("244670316");
    expect(d).toHaveTextContent("PH1234");
    expect(d).toHaveTextContent("Cargo");
    expect(d).toHaveTextContent("12.3 kn · 22.8 km/h");
    expect(d).toHaveTextContent("270°");
    expect(d).toHaveTextContent("NLRTM");
    expect(d).toHaveTextContent("400 m × 58 m");
    expect(d).toHaveTextContent("52.01230, 4.12340");
    expect(mapProps).toHaveBeenLastCalledWith(expect.objectContaining({ selected: 244670316 }));
    fireEvent.click(within(d).getByRole("button", { name: "Close details" }));
    expect(screen.queryByTestId("ais-details")).toBeNull();
  });

  it("filters the list and says so while nothing has a position", async () => {
    render(
      <AisMapView
        events={[
          report(1000, { mmsi: 211000001, name: "ANNA", lat: 54, lon: 8 }),
          report(1000, { mmsi: 211000002, name: "BERTA", lat: 54, lon: 8 }),
        ]}
      />,
    );
    await flush();
    fireEvent.change(screen.getByRole("searchbox", { name: "Search vessels" }), {
      target: { value: "bert" },
    });
    expect(screen.getByTestId("map")).toHaveTextContent("211000002");
    expect(screen.getByTestId("map")).not.toHaveTextContent("211000001");
  });

  it("says so while no vessel has a position", async () => {
    render(<AisMapView events={[report(1000, { mmsi: 211000009 })]} />);
    await flush();
    expect(
      screen.getByText("No vessel matches (or none has sent a position)."),
    ).toBeInTheDocument();
  });
});
