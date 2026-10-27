import { describe, expect, it } from "vitest";
import {
  addrLabel,
  describeGeo,
  describeUa,
  dockerGatewayOnly,
  parseClientInfo,
  viaProxy,
  type AddrInfo,
} from "./clientInfo";

const info = (cls: AddrInfo["class"], over: Partial<AddrInfo> = {}): AddrInfo => ({
  class: cls,
  label: cls,
  family: "ipv4",
  rdns: null,
  country: null,
  countryName: null,
  city: null,
  asn: null,
  org: null,
  ...over,
});

describe("dockerGatewayOnly (banner logic)", () => {
  it("fires when all known addresses are the gateway", () => {
    expect(dockerGatewayOnly([info("docker_gateway"), info("docker_gateway")])).toBe(true);
  });
  it("ignores rows without an address", () => {
    expect(dockerGatewayOnly([null, info("docker_gateway"), null])).toBe(true);
  });
  it("stays quiet without data", () => {
    expect(dockerGatewayOnly([])).toBe(false);
    expect(dockerGatewayOnly([null])).toBe(false);
  });
  it("stays quiet as soon as one other address shows up", () => {
    expect(dockerGatewayOnly([info("docker_gateway"), info("public")])).toBe(false);
    expect(dockerGatewayOnly([info("docker_gateway"), info("private")])).toBe(false);
  });
});

describe("parsing", () => {
  it("reads peer, address info and user agent from a row", () => {
    const c = parseClientInfo({
      peer_addr: "172.22.0.1",
      ip_info: {
        class: "public",
        label: "Public",
        family: "ipv6",
        rdns: "h.example",
        country: "DE",
        asn: 3320,
      },
      peer_info: { class: "docker_gateway", label: "Docker gateway", family: "ipv4" },
      ua_info: { browser: "Firefox", browser_version: "130", os: "Linux", device: "desktop" },
    });
    expect(c.peerAddr).toBe("172.22.0.1");
    expect(c.ipInfo?.family).toBe("ipv6");
    expect(c.ipInfo?.rdns).toBe("h.example");
    expect(c.peerInfo?.class).toBe("docker_gateway");
    expect(c.uaInfo?.browserVersion).toBe("130");
  });
  it("tolerates missing and unknown data", () => {
    const c = parseClientInfo({ ip_info: { class: "martian" }, ua_info: 3 });
    expect(c).toEqual({ peerAddr: null, ipInfo: null, peerInfo: null, uaInfo: null });
  });
});

describe("display helpers", () => {
  it("labels IPv6 addresses", () => {
    expect(addrLabel(info("public", { label: "Public", family: "ipv6" }))).toBe("IPv6 public");
    expect(addrLabel(info("private", { label: "Private LAN" }))).toBe("Private LAN");
  });
  it("describes the browser", () => {
    const ua = { browser: "Chrome", browserVersion: "131", os: "Android", osVersion: "14" };
    expect(describeUa({ ...ua, device: "mobile" }, "raw")).toBe(
      "Chrome 131 on Android 14 (mobile)",
    );
    expect(describeUa({ ...ua, device: "desktop" }, "raw")).toBe("Chrome 131 on Android 14");
    expect(
      describeUa(
        { browser: null, browserVersion: null, os: null, osVersion: null, device: "unknown" },
        "weird/1",
      ),
    ).toBe("weird/1");
    expect(describeUa(null, null)).toBe("Unknown browser");
  });
  it("describes the location", () => {
    expect(
      describeGeo(info("public", { city: "Berlin", country: "DE", asn: 3320, org: "X" })),
    ).toEqual(["Berlin, DE", "AS3320 X"]);
    expect(describeGeo(info("public"))).toEqual([]);
    expect(describeGeo(null)).toEqual([]);
  });
  it("detects forwarding", () => {
    expect(viaProxy("8.8.8.8", "172.22.0.1")).toBe(true);
    expect(viaProxy("172.22.0.1", "172.22.0.1")).toBe(false);
    expect(viaProxy(null, "172.22.0.1")).toBe(false);
  });
});
