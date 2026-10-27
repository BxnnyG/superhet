import { describe, expect, it } from "vitest";
import {
  compareAddresses,
  describeReportedDevice,
  parseReportedClient,
  reportRows,
} from "./clientReport";

const RAW = {
  platform: "Android",
  platform_version: "14.0.0",
  model: "Pixel 8",
  architecture: "arm",
  bitness: "64",
  mobile: true,
  brands: [{ brand: "Chromium", version: "131.0.6778.86" }],
  screen: { width: 412, height: 915, dpr: 2.63 },
  languages: ["de-DE"],
  timezone: "Europe/Berlin",
  touch_points: 5,
  cpu_cores: 8,
  memory_gb: 8,
  connection: { kind: "cellular", effective_type: "4g", downlink_mbps: 9.5, save_data: true },
  public_ip: "203.0.113.0",
  public_ip_info: { class: "reserved", label: "Reserved", family: "ipv4" },
  public_ip_status: "ok",
  reported_at_ms: 1_700_000_000_000,
};

describe("parseReportedClient", () => {
  it("reads every field", () => {
    const r = parseReportedClient(RAW);
    expect(r).toMatchObject({
      platform: "Android",
      model: "Pixel 8",
      mobile: true,
      brands: [{ brand: "Chromium", version: "131.0.6778.86" }],
      screen: { width: 412, height: 915, dpr: 2.63 },
      publicIp: "203.0.113.0",
      publicIpStatus: "ok",
      reportedAtMs: 1_700_000_000_000,
    });
    expect(r?.publicIpInfo?.class).toBe("reserved");
    expect(r?.connection).toEqual({
      kind: "cellular",
      effectiveType: "4g",
      downlinkMbps: 9.5,
      saveData: true,
    });
  });

  it("tolerates a sparse or hostile object", () => {
    expect(parseReportedClient(null)).toBeNull();
    expect(parseReportedClient("x")).toBeNull();
    const r = parseReportedClient({
      languages: [1, "fr", ""],
      brands: [1, { brand: "" }, { brand: "A" }],
      screen: { width: "wide" },
      public_ip_status: "weird",
    });
    expect(r?.languages).toEqual(["fr"]);
    expect(r?.brands).toEqual([{ brand: "A", version: "" }]);
    expect(r?.screen).toBeNull();
    expect(r?.publicIpStatus).toBeNull();
    expect(r?.platform).toBeNull();
  });
});

describe("descriptions", () => {
  it("summarises the device on one line", () => {
    const r = parseReportedClient(RAW);
    expect(r && describeReportedDevice(r)).toBe(
      "Pixel 8 · Android 14.0.0 · arm 64-bit · 412×915 @2.63×",
    );
  });

  it("lists labelled rows, skipping what was not reported", () => {
    const r = parseReportedClient(RAW);
    const rows = r ? reportRows(r) : [];
    const get = (label: string) => rows.find((x) => x.label === label)?.value;
    expect(get("System")).toBe("Android 14.0.0");
    expect(get("Device type")).toBe("Mobile");
    expect(get("Screen")).toBe("412×915, pixel ratio 2.63, 5 touch points");
    expect(get("Languages")).toBe("de-DE");
    expect(get("Hardware")).toBe("8 threads · ≥ 8 GB memory");
    expect(get("Connection")).toBe("cellular · 4g · 9.5 Mbit/s · data saver");
    const sparse = parseReportedClient({ timezone: "UTC" });
    expect(sparse && reportRows(sparse)).toEqual([{ label: "Time zone", value: "UTC" }]);
  });

  it("compares the claimed public address with the one the server sees", () => {
    expect(compareAddresses("1.2.3.4", "1.2.3.4")).toBe("same");
    expect(compareAddresses("1.2.3.4", "192.168.1.5")).toBe("differs");
    expect(compareAddresses(null, "1.2.3.4")).toBeNull();
    expect(compareAddresses("1.2.3.4", null)).toBeNull();
  });
});
