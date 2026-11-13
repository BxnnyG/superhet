import { describe, expect, it } from "vitest";
import {
  DEFAULT_UNITS,
  formatAltFt,
  formatAltRange,
  formatDistanceKm,
  formatSpeedKt,
  formatVrateFpm,
  ftPerAltitudeUnit,
  kmPerDistanceUnit,
  loadUnits,
  saveUnits,
  UNITS_KEY,
  unitNames,
} from "./units";

describe("unit conversion", () => {
  // value in the aviation unit -> text per system (primary) and the tooltip (the other unit)
  it.each([
    ["speed 250 kt", () => formatSpeedKt(250, "metric"), "463 km/h", "250 kt"],
    ["speed 250 kt aviation", () => formatSpeedKt(250, "aviation"), "250 kt", "463 km/h"],
    ["speed 0", () => formatSpeedKt(0, "metric"), "0 km/h", "0 kt"],
    ["altitude FL350", () => formatAltFt(35000, "metric"), "10,668 m", "35,000 ft"],
    ["altitude 1000 ft", () => formatAltFt(1000, "aviation"), "1,000 ft", "305 m"],
    ["altitude below sea level", () => formatAltFt(-100, "metric"), "-30 m", "-100 ft"],
    ["distance 100 km", () => formatDistanceKm(100, "metric"), "100 km", "54.0 NM"],
    ["distance 100 km aviation", () => formatDistanceKm(100, "aviation"), "54.0 NM", "100 km"],
    ["distance 12.34 km", () => formatDistanceKm(12.34, "metric"), "12.3 km", "6.7 NM"],
    ["distance 300 km", () => formatDistanceKm(300, "aviation"), "162 NM", "300 km"],
    ["climb 2000 ft/min", () => formatVrateFpm(2000, "metric"), "+10.2 m/s", "+2,000 ft/min"],
    ["sink 500 ft/min", () => formatVrateFpm(-500, "aviation"), "−500 ft/min", "−2.5 m/s"],
    ["level flight", () => formatVrateFpm(0, "metric"), "0.0 m/s", "0 ft/min"],
  ])("%s", (_name, run, text, other) => {
    expect(run()).toEqual({ text, other });
  });

  it("mixes the way the picture says: feet, km/h, km, ft/min", () => {
    expect(formatAltFt(35000, "mixed").text).toBe("35,000 ft");
    expect(formatSpeedKt(250, "mixed").text).toBe("463 km/h");
    expect(formatDistanceKm(100, "mixed").text).toBe("100 km");
    expect(formatVrateFpm(500, "mixed").text).toBe("+500 ft/min");
    // The tooltip is always the opposite unit of the same quantity.
    expect(formatAltFt(35000, "mixed").other).toBe("10,668 m");
    expect(formatSpeedKt(250, "mixed").other).toBe("250 kt");
  });

  it("names the units and the scale factors", () => {
    expect(unitNames("metric")).toEqual({ speed: "km/h", alt: "m", dist: "km" });
    expect(unitNames("aviation")).toEqual({ speed: "kt", alt: "ft", dist: "NM" });
    expect(unitNames("mixed")).toEqual({ speed: "km/h", alt: "ft", dist: "km" });
    expect(kmPerDistanceUnit("aviation")).toBe(1.852);
    expect(kmPerDistanceUnit("metric")).toBe(1);
    expect(ftPerAltitudeUnit("aviation")).toBe(1);
    expect(ftPerAltitudeUnit("metric")).toBeCloseTo(3.2808, 3);
  });

  it("writes altitude bands as round numbers in metres", () => {
    expect(formatAltRange(null, 5000, "aviation")).toBe("ground to 5,000 ft");
    expect(formatAltRange(5000, 10000, "metric")).toBe("1,500 to 3,000 m");
    expect(formatAltRange(40000, null, "mixed")).toBe("above 40,000 ft");
    expect(formatAltRange(null, null, "metric")).toBe("all altitudes");
  });
});

describe("the units setting", () => {
  const storage = (value: string | null) => ({ getItem: () => value });

  it("defaults to metric and ignores garbage", () => {
    expect(DEFAULT_UNITS).toBe("metric");
    expect(loadUnits(storage(null))).toBe("metric");
    expect(loadUnits(storage("furlongs"))).toBe("metric");
  });

  it("is kept in the browser and read back", () => {
    const store = new Map<string, string>();
    saveUnits("aviation", { setItem: (k, v) => void store.set(k, v) });
    expect(store.get(UNITS_KEY)).toBe("aviation");
    expect(loadUnits(storage(store.get(UNITS_KEY) ?? null))).toBe("aviation");
    expect(loadUnits(storage("mixed"))).toBe("mixed");
  });

  it("survives storage that throws", () => {
    const broken = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    };
    expect(loadUnits(broken)).toBe("metric");
    expect(() => saveUnits("aviation", broken)).not.toThrow();
  });
});
