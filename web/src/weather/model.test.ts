import { describe, expect, it } from "vitest";
import {
  ageText,
  clock,
  compass,
  countdown,
  dayLabel,
  imageUrl,
  parseCapture,
  parseConfig,
  parsePass,
  parsePasses,
  primaryHz,
  quality,
} from "./model";

const PASS = {
  satellite: "METEOR-M2 4",
  norad_id: 59051,
  format: "LRPT",
  aos_ms: 1_000_000,
  tca_ms: 1_300_000,
  los_ms: 1_600_000,
  max_elevation_deg: 56.4,
  aos_azimuth_deg: 10,
  tca_azimuth_deg: 59,
  los_azimuth_deg: 200,
  in_progress: false,
  downlinks: [{ hz: 137_900_000, label: "LRPT 137.900 MHz" }],
  elements_age_h: 5.5,
};

describe("weather model", () => {
  it("parses a pass and rejects incomplete ones", () => {
    const p = parsePass(PASS);
    expect(p?.satellite).toBe("METEOR-M2 4");
    expect(p?.maxElevationDeg).toBe(56.4);
    expect(primaryHz(p ?? { downlinks: [] })).toBe(137_900_000);
    expect(parsePass({ ...PASS, aos_ms: "x" })).toBeNull();
    expect(parsePass(null)).toBeNull();
    const reply = parsePasses({ passes: [PASS, { junk: 1 }], position_from: "adsb" });
    expect(reply.passes).toHaveLength(1);
    expect(reply.positionFrom).toBe("adsb");
  });

  it("parses the config and keeps the decommissioned status", () => {
    const c = parseConfig({
      satellites: [
        { norad_id: 59051, name: "METEOR-M2 4", status: "operational", downlinks: [], note: "" },
        { norad_id: 33591, name: "NOAA 19", status: "decommissioned", note: "off", downlinks: [] },
        { norad_id: 1, name: "bad", status: "unknown" },
      ],
      checked: "2026-11-23",
      elements: { enabled: true, source: "download", fetched_ms: 5, satellites: [] },
      store: { available: true, captures: 2, used_bytes: 10, max_bytes: 100 },
    });
    expect(c.satellites.map((s) => s.status)).toEqual(["operational", "decommissioned"]);
    expect(c.elements.enabled).toBe(true);
    expect(c.elements.source).toBe("download");
    expect(c.store.captures).toBe(2);
    expect(parseConfig(undefined).satellites).toEqual([]);
  });

  it("parses captures", () => {
    const c = parseCapture({
      id: "1700000100-0001",
      satellite: null,
      pipeline: "apt",
      started_ms: 5,
      keep: true,
      images: [{ name: "a.png", thumb: "thumb-a.png", width: 1, height: 2 }, { x: 1 }],
    });
    expect(c?.pipeline).toBe("apt");
    expect(c?.satellite).toBeNull();
    expect(c?.keep).toBe(true);
    expect(c?.images).toHaveLength(1);
    expect(c?.images[0]?.label).toBe("a.png");
    expect(parseCapture({ started_ms: 1 })).toBeNull();
  });

  it("counts down to a pass", () => {
    const pass = { aosMs: 10_000_000, losMs: 10_600_000 };
    expect(countdown(pass, 10_000_000 - 30_000)).toBe("in 30 s");
    expect(countdown(pass, 10_000_000 - 12 * 60_000)).toBe("in 12 min");
    expect(countdown(pass, 10_000_000 - 125 * 60_000)).toBe("in 2 h 05 min");
    expect(countdown(pass, 10_000_000 - 120 * 60_000)).toBe("in 2 h");
    expect(countdown(pass, 10_300_000)).toBe("overhead now");
    expect(countdown(pass, 10_600_000)).toBe("passed");
  });

  it("names compass points and rates passes", () => {
    expect(compass(0)).toBe("N");
    expect(compass(59)).toBe("ENE");
    expect(compass(180)).toBe("S");
    expect(compass(359)).toBe("N");
    expect(compass(-90)).toBe("W");
    expect(quality(8)).toBe("poor");
    expect(quality(15)).toBe("fair");
    expect(quality(30)).toBe("good");
    expect(quality(75)).toBe("excellent");
  });

  it("formats times and ages", () => {
    const noon = new Date(2026, 10, 23, 12, 5).getTime();
    expect(clock(noon)).toBe("12:05");
    expect(dayLabel(noon, noon + 3_600_000)).toBeNull();
    expect(dayLabel(noon, noon + 2 * 86_400_000)).toMatch(/Mon/);
    expect(ageText(0, 5 * 3_600_000)).toBe("5 h");
    expect(ageText(0, 72 * 3_600_000)).toBe("3 d");
  });

  it("builds image URLs that cannot leave the capture", () => {
    expect(imageUrl("1700000100-0001", "a b.png")).toBe(
      "/api/v1/weather/captures/1700000100-0001/images/a%20b.png",
    );
    expect(imageUrl("1700000100-0001", "a.png", { thumb: true, download: true })).toBe(
      "/api/v1/weather/captures/1700000100-0001/images/a.png?thumb=true&download=true",
    );
    expect(imageUrl("x", "../y.png")).toContain("..%2Fy.png");
  });
});
