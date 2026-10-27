import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ClientReportNotice } from "../auth/GuestPage";
import { parseStatus } from "../auth/api";
import {
  collectDeviceInfo,
  fetchPublicIp,
  parseIpAnswer,
  runClientReport,
  type ClientReportPayload,
  type DeviceEnv,
} from "./collect";
import { useClientReport } from "./useClientReport";

function env(over: Partial<DeviceEnv["navigator"]> = {}, rest: Partial<DeviceEnv> = {}): DeviceEnv {
  return {
    navigator: {
      platform: "Linux x86_64",
      languages: ["de-DE", "en"],
      language: "de-DE",
      maxTouchPoints: 0,
      hardwareConcurrency: 8,
      deviceMemory: 8,
      ...over,
    },
    screen: { width: 2560, height: 1440 },
    devicePixelRatio: 1.25,
    timeZone: () => "Europe/Berlin",
    ...rest,
  };
}

const chromium = {
  mobile: false,
  platform: "Linux",
  getHighEntropyValues: vi.fn().mockResolvedValue({
    platformVersion: "6.12.0",
    model: "",
    architecture: "x86",
    bitness: "64",
    fullVersionList: [
      { brand: "Not A;Brand", version: "99.0.0.0" },
      { brand: "Chromium", version: "131.0.6778.86" },
    ],
  }),
};

describe("collectDeviceInfo", () => {
  it("collects Client Hints, screen, languages, zone, hardware and connection", async () => {
    const out = await collectDeviceInfo(
      env({
        userAgentData: chromium,
        connection: { type: "wifi", effectiveType: "4g", downlink: 10, saveData: false },
      }),
    );
    expect(chromium.getHighEntropyValues).toHaveBeenCalledWith([
      "platformVersion",
      "model",
      "architecture",
      "bitness",
      "fullVersionList",
    ]);
    expect(out).toEqual({
      platform: "Linux",
      mobile: false,
      platform_version: "6.12.0",
      architecture: "x86",
      bitness: "64",
      brands: [{ brand: "Chromium", version: "131.0.6778.86" }],
      screen: { width: 2560, height: 1440, dpr: 1.25 },
      languages: ["de-DE", "en"],
      timezone: "Europe/Berlin",
      touch_points: 0,
      cpu_cores: 8,
      memory_gb: 8,
      connection: { kind: "wifi", effective_type: "4g", downlink_mbps: 10, save_data: false },
    });
  });

  it("falls back where Client Hints do not exist (Firefox, Safari) and leaves gaps out", async () => {
    const out = await collectDeviceInfo(
      env({ deviceMemory: undefined, hardwareConcurrency: 4 }, { screen: undefined }),
    );
    expect(out.platform).toBe("Linux x86_64");
    expect(out.brands).toBeUndefined();
    expect(out.screen).toBeUndefined();
    expect(out.memory_gb).toBeUndefined();
    expect(out.connection).toBeUndefined();
    expect(out.cpu_cores).toBe(4);
    expect(out.languages).toEqual(["de-DE", "en"]);
  });

  it("survives refusing or hanging high entropy hints", async () => {
    vi.useFakeTimers();
    const refusing = { platform: "Android", getHighEntropyValues: vi.fn().mockRejectedValue(1) };
    expect((await collectDeviceInfo(env({ userAgentData: refusing }))).platform).toBe("Android");
    const hanging = {
      platform: "Windows",
      getHighEntropyValues: () => new Promise<never>(() => {}),
    };
    const pending = collectDeviceInfo(env({ userAgentData: hanging }));
    await vi.advanceTimersByTimeAsync(2000);
    expect((await pending).platform).toBe("Windows");
    vi.useRealTimers();
  });

  it("uses the single language when the list is missing", async () => {
    const out = await collectDeviceInfo(env({ languages: undefined, language: "fr" }));
    expect(out.languages).toEqual(["fr"]);
  });
});

describe("public IP lookup", () => {
  const answer = (body: string, status = 200) =>
    vi.fn().mockResolvedValue(new Response(body, { status }));

  it("reads JSON and plain answers", () => {
    expect(parseIpAnswer('{"ip":"203.0.113.7"}')).toBe("203.0.113.7");
    expect(parseIpAnswer(" 2001:db8::1\n")).toBe("2001:db8::1");
    for (const bad of ["", "<html>", '{"ip":"x"}', '{"ip":5}', "not json {", "12345", "a;b:c"]) {
      expect(parseIpAnswer(bad), bad).toBeNull();
    }
  });

  it("asks without cookies, referrer or cache", async () => {
    const f = answer('{"ip":"203.0.113.7"}');
    const r = await fetchPublicIp("https://api.ipify.org?format=json", f);
    expect(r).toEqual({ ip: "203.0.113.7", status: "ok" });
    expect(f).toHaveBeenCalledWith(
      "https://api.ipify.org?format=json",
      expect.objectContaining({
        credentials: "omit",
        referrerPolicy: "no-referrer",
        cache: "no-store",
      }),
    );
  });

  it("tells a blocked request from a bad answer", async () => {
    const blocked = vi.fn().mockRejectedValue(new TypeError("Failed to fetch"));
    expect(await fetchPublicIp("https://x.example", blocked)).toEqual({
      ip: null,
      status: "blocked",
    });
    expect(await fetchPublicIp("https://x.example", answer("nope"))).toEqual({
      ip: null,
      status: "failed",
    });
    expect(await fetchPublicIp("https://x.example", answer("{}", 500))).toEqual({
      ip: null,
      status: "failed",
    });
  });

  it("gives up after the timeout", async () => {
    vi.useFakeTimers();
    const hang = vi.fn(
      (_url: unknown, init?: RequestInit) =>
        new Promise<Response>((_, reject) => {
          init?.signal?.addEventListener("abort", () =>
            reject(new DOMException("aborted", "AbortError")),
          );
        }),
    );
    const p = fetchPublicIp("https://x.example", hang as unknown as typeof fetch, 1000);
    await vi.advanceTimersByTimeAsync(1100);
    expect(await p).toEqual({ ip: null, status: "failed" });
    vi.useRealTimers();
  });
});

describe("runClientReport", () => {
  it("sends device details only when no lookup is asked for", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const fetchImpl = vi.fn();
    await runClientReport({ publicIpService: null }, { env: env(), send, fetchImpl });
    expect(fetchImpl).not.toHaveBeenCalled();
    const payload = send.mock.calls[0]?.[0] as ClientReportPayload;
    expect(payload.platform).toBe("Linux x86_64");
    expect(payload.public_ip).toBeUndefined();
    expect(payload.public_ip_status).toBeUndefined();
  });

  it("adds the public address and its status when the lookup is asked for", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const fetchImpl = vi.fn().mockResolvedValue(new Response('{"ip":"198.51.100.4"}'));
    await runClientReport(
      { publicIpService: "https://api.ipify.org?format=json" },
      { env: env(), send, fetchImpl },
    );
    expect(send.mock.calls[0]?.[0]).toMatchObject({
      public_ip: "198.51.100.4",
      public_ip_status: "ok",
    });
  });

  it("still reports the device when the lookup is blocked, and never throws", async () => {
    const send = vi.fn().mockResolvedValue(undefined);
    const fetchImpl = vi.fn().mockRejectedValue(new TypeError("blocked"));
    await runClientReport(
      { publicIpService: "https://x.example" },
      { env: env(), send, fetchImpl },
    );
    expect(send.mock.calls[0]?.[0]).toMatchObject({ public_ip_status: "blocked" });
    expect(send.mock.calls[0]?.[0]).not.toHaveProperty("public_ip");
    const failing = vi.fn().mockRejectedValue(new Error("server said no"));
    await expect(
      runClientReport({ publicIpService: null }, { env: env(), send: failing }),
    ).resolves.toBeNull();
  });
});

describe("useClientReport", () => {
  function Probe(props: { ask: { publicIpService: string | null } | null; who: string | null }) {
    useClientReport(props.ask, props.who, deps);
    return null;
  }
  const send = vi.fn().mockResolvedValue(undefined);
  const deps = { env: env(), send, fetchImpl: vi.fn() };

  it("reports once per identity, again for another account, never when not asked", async () => {
    send.mockClear();
    const { rerender } = render(<Probe ask={null} who="user:1" />);
    await Promise.resolve();
    expect(send).not.toHaveBeenCalled();
    rerender(<Probe ask={{ publicIpService: null }} who="user:1" />);
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    rerender(<Probe ask={{ publicIpService: null }} who="user:1" />);
    await Promise.resolve();
    expect(send).toHaveBeenCalledTimes(1);
    rerender(<Probe ask={{ publicIpService: null }} who="user:2" />);
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(2));
    // Signed out and back in as the same account: a new session, a new report.
    rerender(<Probe ask={null} who={null} />);
    rerender(<Probe ask={{ publicIpService: null }} who="user:2" />);
    await vi.waitFor(() => expect(send).toHaveBeenCalledTimes(3));
  });
});

describe("status and guest notice", () => {
  const base = { setup_required: false, authenticated: true, kind: "user", role: "admin" };

  it("reads what the server asks for", () => {
    const asked = parseStatus({
      ...base,
      client_report: { public_ip_service: "https://api.ipify.org?format=json" },
    });
    expect(asked.clientReport).toEqual({ publicIpService: "https://api.ipify.org?format=json" });
    expect(parseStatus({ ...base, client_report: {} }).clientReport).toEqual({
      publicIpService: null,
    });
    expect(parseStatus({ ...base, client_report: null }).clientReport).toBeUndefined();
    // A lookup URL that is not https is never followed.
    expect(
      parseStatus({ ...base, client_report: { public_ip_service: "http://x.example" } })
        .clientReport,
    ).toEqual({ publicIpService: null });
    expect(
      parseStatus({
        setup_required: false,
        authenticated: false,
        client_report: { public_ip_service: null },
      }).clientReport,
    ).toBeUndefined();
  });

  it("tells guests up front, naming the service host", () => {
    const { container, rerender } = render(
      <ClientReportNotice request={{ publicIpService: "https://api.ipify.org?format=json" }} />,
    );
    expect(container.textContent).toMatch(/device details/);
    expect(container.textContent).toMatch(/public IP address/);
    expect(container.textContent).toMatch(/api\.ipify\.org/);
    rerender(<ClientReportNotice request={{ publicIpService: null }} />);
    expect(container.textContent).not.toMatch(/public IP address/);
  });
});
