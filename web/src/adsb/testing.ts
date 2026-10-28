// Fixtures for the ADS-B view tests: the plugin's status document and reply as the API sends
// them, and a fake API.

import { vi } from "vitest";
import type { AdsbApi } from "./api";
import { parseReply, type AdsbReply } from "./model";

export function aircraftRaw(over: Record<string, unknown> = {}) {
  return {
    icao: "3c6444",
    callsign: "DLH123",
    alt_ft: 35000,
    speed_kt: 451.3,
    track_deg: 123.5,
    vrate_fpm: -64,
    lat: 51.12345,
    lon: 6.98765,
    rssi: -12.3,
    squawk: "1000",
    category: "A3",
    messages: 812,
    seen_s: 0.2,
    seen_pos_s: 0.4,
    ...over,
  };
}

export function adsbStatusRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    mode: "running",
    updated_ms: 1790000000000,
    stale_after_s: 60,
    receiver: { lat: 51, lon: 7 },
    messages: 4821,
    messages_per_s: 565,
    total: 3,
    with_position: 2,
    truncated: false,
    error: null,
    aircraft: [
      aircraftRaw(),
      aircraftRaw({
        icao: "406a3d",
        callsign: "EZY45K",
        alt_ft: undefined,
        on_ground: true,
        speed_kt: 12,
        track_deg: 270,
        vrate_fpm: undefined,
        lat: 50.86569,
        lon: 7.14221,
        rssi: -8,
        seen_s: 0.5,
      }),
      {
        icao: "4ca7b2",
        rssi: -21.4,
        messages: 14,
        seen_s: 3.1,
      },
    ],
    ...over,
  };
}

export function adsbReplyRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    ok: true,
    plugin_id: "org.superhet.decoder.adsb",
    device: "dev0",
    state: "running",
    freq_hz: 1090000000,
    audio_vfo: null,
    status: adsbStatusRaw(),
    updated_unix_ms: 1790000000000,
    media: [],
    ...over,
  };
}

export const adsbReply = (over: Record<string, unknown> = {}): AdsbReply =>
  parseReply(adsbReplyRaw(over));

export function fakeAdsbApi(initial: AdsbReply = adsbReply()) {
  const state = { reply: initial };
  const api = {
    status: vi.fn(() => Promise.resolve(state.reply)),
  } satisfies AdsbApi;
  return { api, state };
}
