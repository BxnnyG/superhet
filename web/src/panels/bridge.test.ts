import { describe, expect, it } from "vitest";
import type { DecoderEvent } from "../decoders/events";
import type { PluginInstance } from "../plugins/api";
import {
  MAX_PANEL_EVENTS,
  MAX_TUNE_HZ,
  THEME_TOKENS,
  TuneLimiter,
  fromPanelFrame,
  mayTune,
  panelEvents,
  panelInstances,
  parseGuestMessage,
  readTheme,
  tuneResultMessage,
} from "./bridge";

const hello = { superhet: 1, type: "hello" };
const tune = (freq_hz: unknown) => ({ superhet: 1, type: "tune", freq_hz });

describe("parseGuestMessage", () => {
  it("accepts exactly the two messages of the contract", () => {
    expect(parseGuestMessage(hello)).toEqual({ type: "hello" });
    expect(parseGuestMessage(tune(100_000_000))).toEqual({ type: "tune", freqHz: 100_000_000 });
    expect(parseGuestMessage(tune(1))).toEqual({ type: "tune", freqHz: 1 });
    expect(parseGuestMessage(tune(MAX_TUNE_HZ))).toEqual({ type: "tune", freqHz: MAX_TUNE_HZ });
  });

  it("rejects anything else, silently", () => {
    for (const bad of [
      null,
      undefined,
      "hello",
      42,
      [],
      [hello],
      {},
      { type: "hello" },
      { superhet: 2, type: "hello" },
      { superhet: "1", type: "hello" },
      { superhet: 1 },
      { superhet: 1, type: "unknown" },
      { superhet: 1, type: "hello", extra: true },
      { superhet: 1, type: "tune" },
      { ...tune(1), extra: 1 },
      // The API is not reachable through the bridge, whatever it is asked.
      { superhet: 1, type: "fetch", url: "/api/v1/devices" },
      { superhet: 1, type: "request", path: "/api/v1/plugins", method: "DELETE" },
    ]) {
      expect(parseGuestMessage(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it("validates the frequency", () => {
    for (const f of [0, -1, 1.5, NaN, Infinity, MAX_TUNE_HZ + 1, 2 ** 60, "100", null, [], {}]) {
      expect(parseGuestMessage(tune(f)), String(f)).toBeNull();
    }
  });

  it("refuses objects with a foreign prototype", () => {
    class Evil {
      superhet = 1;
      type = "hello";
    }
    expect(parseGuestMessage(new Evil())).toBeNull();
    expect(parseGuestMessage(Object.assign(Object.create(null), hello))).toEqual({ type: "hello" });
  });
});

describe("fromPanelFrame", () => {
  const frame = {} as Window;
  it("believes the frame's window with the opaque origin and nobody else", () => {
    expect(fromPanelFrame({ source: frame, origin: "null" }, frame)).toBe(true);
    // Another window (a popup, the app itself, another frame).
    expect(fromPanelFrame({ source: {} as Window, origin: "null" }, frame)).toBe(false);
    expect(fromPanelFrame({ source: null, origin: "null" }, frame)).toBe(false);
    // The frame navigated to a real origin: not the sandboxed page any more.
    expect(fromPanelFrame({ source: frame, origin: "https://evil.example" }, frame)).toBe(false);
    expect(fromPanelFrame({ source: frame, origin: location.origin }, frame)).toBe(false);
    expect(fromPanelFrame({ source: frame, origin: "null" }, null)).toBe(false);
    expect(fromPanelFrame({ source: null, origin: "null" }, null)).toBe(false);
  });
});

describe("role gating", () => {
  it("tunes only for operators and only if the manifest asks for it", () => {
    expect(mayTune(true, true)).toBe(true);
    expect(mayTune(true, false)).toBe(false);
    expect(mayTune(false, true)).toBe(false);
    expect(mayTune(false, false)).toBe(false);
  });

  it("limits the rate of tune requests", () => {
    let now = 10_000;
    const limiter = new TuneLimiter(() => now, 1000);
    expect(limiter.allow()).toBe(true);
    expect(limiter.allow()).toBe(false);
    now += 999;
    expect(limiter.allow()).toBe(false);
    now += 1;
    expect(limiter.allow()).toBe(true);
  });

  it("answers a refusal with a reason and a success without one", () => {
    expect(tuneResultMessage(false, "not allowed")).toEqual({
      superhet: 1,
      type: "tune_result",
      ok: false,
      error: "not allowed",
    });
    expect(tuneResultMessage(true)).toEqual({ superhet: 1, type: "tune_result", ok: true });
  });
});

const instance = (over: Partial<PluginInstance>): PluginInstance =>
  ({
    id: "i1",
    pluginId: "org.superhet.example.echo",
    device: "dev0",
    vfo: 1,
    freqHz: 144_800_000,
    config: { callsign: "SECRET", passcode: "1234" },
    exclusive: false,
    state: "running",
    pid: 4242,
    restarts: 1,
    crashesInWindow: 0,
    retryInMs: null,
    lastError: "/data/plugins/x: boom",
    eventsOk: 3,
    eventsInvalid: 0,
    inputDropped: 0,
    inputError: null,
    updatedMs: 5,
    inputBytes: 10,
    inputLevelDb: -30,
    lastInputMs: 1,
    lastEventMs: 2,
    createdMs: 0,
    reconfigures: 0,
    audioVfo: null,
    ...over,
  }) as PluginInstance;

describe("what a panel may read", () => {
  it("gets instances of the plugins it declared, without configuration or internals", () => {
    const shown = panelInstances(
      [
        instance({}),
        instance({ id: "i2", pluginId: "org.superhet.decoder.aprs" }),
        instance({ id: "i3", state: "stopped" }),
      ],
      ["org.superhet.example.echo"],
    );
    expect(shown).toEqual([
      {
        id: "i1",
        plugin_id: "org.superhet.example.echo",
        device: "dev0",
        vfo: 1,
        freq_hz: 144_800_000,
        state: "running",
        updated_ms: 5,
      },
    ]);
    const text = JSON.stringify(shown);
    for (const secret of ["SECRET", "passcode", "4242", "boom", "config", "pid"]) {
      expect(text).not.toContain(secret);
    }
  });

  const event = (id: number, source: string): DecoderEvent => ({
    id,
    tsMs: id * 1000,
    device: "dev0",
    vfo: null,
    freqHz: null,
    mode: null,
    kind: "echo.block",
    source,
    value: `v${id}`,
    meta: { seq: id },
  });

  it("gets the events of its plugins only, newest first, capped", () => {
    const all: DecoderEvent[] = [];
    for (let i = 200; i > 0; i--) {
      all.push(event(i, i % 2 === 0 ? "org.superhet.example.echo" : "core"));
    }
    const shown = panelEvents(all, ["org.superhet.example.echo"]);
    expect(shown).toHaveLength(MAX_PANEL_EVENTS);
    expect(shown[0]?.id).toBe(200);
    expect(shown.every((e) => e.source === "org.superhet.example.echo")).toBe(true);
    expect(panelEvents(all, [])).toEqual([]);
  });
});

describe("readTheme", () => {
  it("passes the app's colours and the colour scheme, dropping odd values", () => {
    const root = document.documentElement;
    root.style.setProperty("--fg", "#112233");
    root.style.setProperty("--accent", "oklch(0.6 0.2 250)");
    root.style.setProperty("--danger", "red; background: url(//evil)");
    const theme = readTheme(root);
    expect(theme.fg).toBe("#112233");
    expect(theme.accent).toBe("oklch(0.6 0.2 250)");
    expect(theme.danger).toBeUndefined();
    expect(["light", "dark"]).toContain(theme.scheme);
    for (const key of Object.keys(theme)) {
      expect([...THEME_TOKENS, "scheme"]).toContain(key);
    }
    root.style.removeProperty("--fg");
    root.style.removeProperty("--accent");
    root.style.removeProperty("--danger");
  });
});
