// Fixtures for the ADS-B view tests: the plugin's status document and reply as the API sends
// them, and a fake API.

import { vi } from "vitest";
import type { AdsbApi } from "./api";
import { parseDatabaseStatus, parseLookup, type DatabaseStatus } from "./info";
import { parseReply, type AdsbReply } from "./model";
import { parseHistory, parseOnline, parseOnlineAdmin, type OnlineSettings } from "./online";
import { parseRange } from "./range";

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

/** `GET /api/v1/adsb/lookup` for the aircraft of [`adsbStatusRaw`]. */
export function lookupRaw() {
  return {
    v: 1,
    aircraft: {
      "3c6444": {
        reg: "D-AIXA",
        type: "A359",
        type_desc: "AIRBUS A-350-900",
        class: "L2J",
        wake: "H",
        military: false,
      },
      "406a3d": {
        reg: "G-EZTH",
        type: "A320",
        type_desc: "AIRBUS A-320",
        class: "L2J",
        wake: "M",
        military: false,
      },
    },
    operators: { DLH: { name: "Lufthansa", country: "Germany", callsign: "LUFTHANSA" } },
  };
}

/** `GET /api/v1/adsb/range`: a few sectors and some activity. */
export function rangeRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    device: "dev0",
    receiver: { lat: 51, lon: 7 },
    since_ms: 1_789_000_000_000,
    sector_deg: 5,
    bands: 6,
    cells: [
      [0, 4, 180.5, "3c6444"],
      [18, 2, 95.25, "406a3d"],
      [18, 4, 210, "3c6445"],
      [40, 0, 12.5, ""],
    ],
    bucket_s: 120,
    series: [
      [1_789_999_880_000, 110, 8, 5],
      [1_790_000_000_000, 140, 12, 9],
    ],
    ...over,
  };
}

export function databaseStatusRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    database: {
      source: "Mictronics aircraft database",
      source_url: "https://example.org/aircraft-database",
      licence: "ODC-By 1.0",
      licence_url: "https://opendatacommons.org/licenses/by/1-0/",
      version: "530",
      origin: "download",
      aircraft: 455928,
      types: 2788,
      operators: 6310,
      imported_ms: 1_789_000_000_000,
    },
    offer: {
      source: "Mictronics aircraft database",
      source_url: "https://example.org/aircraft-database",
      licence: "ODC-By 1.0",
      licence_url: "https://opendatacommons.org/licenses/by/1-0/",
      download_url: "https://example.org/aircraft-database/indexedDB.zip",
      file: null,
      archive: true,
    },
    job: { state: "idle", message: "", finished_ms: null },
    ...over,
  };
}

export const databaseStatus = (over: Record<string, unknown> = {}): DatabaseStatus => {
  const parsed = parseDatabaseStatus(databaseStatusRaw(over));
  if (parsed === null) throw new Error("fixture");
  return parsed;
};

/** The reply of `GET /api/v1/adsb/online`; off by default, like a fresh install. */
export function onlineRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    enabled: false,
    locked: false,
    sources: { adsbdb: "off", planespotters: "off" },
    aircraft: null,
    route: null,
    photo: null,
    ...over,
  };
}

/** The same with both sources on and everything found (DLH123 Frankfurt to New York). */
export function onlineFullRaw(over: Record<string, unknown> = {}) {
  return onlineRaw({
    enabled: true,
    sources: { adsbdb: "ok", planespotters: "ok" },
    aircraft: {
      registration: "D-AIXP",
      type: "A350 941",
      icao_type: "A359",
      manufacturer: "Airbus",
      owner: "Lufthansa",
      owner_country: "Germany",
      owner_country_iso: "DE",
    },
    route: {
      callsign: "DLH123",
      callsign_iata: "LH123",
      airline: { name: "Lufthansa", icao: "DLH", iata: "LH", country: "Germany" },
      origin: {
        name: "Frankfurt am Main Airport",
        icao: "EDDF",
        iata: "FRA",
        city: "Frankfurt am Main",
        country: "Germany",
        country_iso: "DE",
        lat: 50.03333,
        lon: 8.57056,
      },
      destination: {
        name: "John F Kennedy International Airport",
        icao: "KJFK",
        iata: "JFK",
        city: "New York",
        country: "United States",
        country_iso: "US",
        lat: 40.6398,
        lon: -73.7789,
      },
    },
    photo: {
      src: "https://t.plnspttrs.net/05121/1053982_2810c547e6_280.jpg",
      link: "https://www.planespotters.net/photo/1053982/d-aixp-lufthansa-airbus-a350-941",
      photographer: "Günther Feniuk",
      width: 395,
      height: 280,
    },
    ...over,
  });
}

export function historyRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    seen: true,
    events: 14,
    days: 3,
    first_ms: 1_789_000_000_000,
    last_ms: 1_789_900_000_000,
    callsigns: [
      { callsign: "DLH123", events: 9, last_ms: 1_789_900_000_000 },
      { callsign: "DLH456", events: 5, last_ms: 1_789_500_000_000 },
    ],
    ...over,
  };
}

export const onlineSettingsRaw = (
  over: { settings?: Partial<OnlineSettings>; locked?: boolean } = {},
) => {
  const settings = { enabled: false, adsbdb: false, planespotters: false, ...over.settings };
  return {
    v: 1,
    settings,
    effective: settings,
    locked: over.locked ?? false,
    env: "SUPERHET_ADSB_ONLINE",
    sources: [
      {
        id: "adsbdb",
        name: "adsbdb.com",
        url: "https://www.adsbdb.com",
        provides: "full type name, registered owner, route and airline",
        sends: "the aircraft's ICAO address and callsign",
      },
      {
        id: "planespotters",
        name: "Planespotters.net",
        url: "https://www.planespotters.net",
        provides: "a photo of the aircraft with the photographer's credit",
        sends: "the aircraft's ICAO address",
      },
    ],
  };
};

export function fakeAdsbApi(initial: AdsbReply = adsbReply()) {
  const state = {
    reply: initial,
    database: databaseStatus(),
    online: onlineRaw() as Record<string, unknown>,
    history: null as Record<string, unknown> | null,
    onlineAdmin: onlineSettingsRaw() as ReturnType<typeof onlineSettingsRaw>,
  };
  const api = {
    status: vi.fn(() => Promise.resolve(state.reply)),
    lookup: vi.fn(() => Promise.resolve(parseLookup(lookupRaw()))),
    range: vi.fn(() => {
      const data = parseRange(rangeRaw());
      return data === null ? Promise.reject(new Error("fixture")) : Promise.resolve(data);
    }),
    resetRange: vi.fn(() => Promise.resolve()),
    database: vi.fn(() => Promise.resolve(state.database)),
    updateDatabase: vi.fn(() => Promise.resolve()),
    removeDatabase: vi.fn(() => Promise.resolve()),
    online: vi.fn<AdsbApi["online"]>(() => {
      const reply = parseOnline(state.online);
      return reply === null ? Promise.reject(new Error("fixture")) : Promise.resolve(reply);
    }),
    history: vi.fn<AdsbApi["history"]>(() => Promise.resolve(parseHistory(state.history))),
    onlineSettings: vi.fn(() => {
      const admin = parseOnlineAdmin(state.onlineAdmin);
      return admin === null ? Promise.reject(new Error("fixture")) : Promise.resolve(admin);
    }),
    setOnlineSettings: vi.fn((settings: OnlineSettings) => {
      state.onlineAdmin = onlineSettingsRaw({ settings, locked: state.onlineAdmin.locked });
      return Promise.resolve();
    }),
  } satisfies AdsbApi;
  return { api, state };
}
