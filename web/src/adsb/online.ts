// Online aircraft details (ADR-0033): what `GET /api/v1/adsb/online` returns once an admin has
// switched the sources on, and the "seen before" statistics of `GET /api/v1/adsb/history`.
// Everything here is parsed defensively: the API passes on what public services answered.

import { isRec } from "../spectrum/api";

export type SourceState = "off" | "ok" | "not_found" | "unavailable" | "rate_limited";
const SOURCE_STATES: readonly string[] = ["off", "ok", "not_found", "unavailable", "rate_limited"];

export interface OnlineAirport {
  name: string;
  icao: string | null;
  iata: string | null;
  city: string | null;
  country: string | null;
  countryIso: string | null;
  lat: number | null;
  lon: number | null;
}

export interface OnlineRoute {
  origin: OnlineAirport;
  destination: OnlineAirport;
  midpoint: OnlineAirport | null;
  callsign: string | null;
  callsignIata: string | null;
  airline: {
    name: string | null;
    icao: string | null;
    iata: string | null;
    country: string | null;
  } | null;
}

export interface OnlineAircraft {
  registration: string | null;
  /** Full type name, e.g. "A350 941". */
  type: string | null;
  icaoType: string | null;
  manufacturer: string | null;
  owner: string | null;
  ownerCountry: string | null;
  ownerCountryIso: string | null;
}

export interface OnlinePhoto {
  /** Image URL, used unchanged (the photo service's terms). */
  src: string;
  /** The photo's page at the service; the thumbnail must lead there. */
  link: string;
  photographer: string;
  width: number | null;
  height: number | null;
}

export interface OnlineReply {
  /** The admin switched online details on. */
  enabled: boolean;
  /** `SUPERHET_ADSB_ONLINE` decides; the admin console cannot change it. */
  locked: boolean;
  adsbdb: SourceState;
  planespotters: SourceState;
  aircraft: OnlineAircraft | null;
  route: OnlineRoute | null;
  photo: OnlinePhoto | null;
}

/** Hosts a photo may be loaded from (the server checks too; the page CSP allows exactly these). */
export const PHOTO_HOSTS: readonly string[] = ["t.plnspttrs.net", "cdn.planespotters.net"];
const PHOTO_PAGE_HOST = "www.planespotters.net";

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

function httpsOn(url: string, hosts: readonly string[]): boolean {
  try {
    const u = new URL(url);
    return (
      u.protocol === "https:" && u.port === "" && u.username === "" && hosts.includes(u.hostname)
    );
  } catch {
    return false;
  }
}

const state = (v: unknown): SourceState =>
  typeof v === "string" && SOURCE_STATES.includes(v) ? (v as SourceState) : "unavailable";

function airport(raw: unknown): OnlineAirport | null {
  if (!isRec(raw)) return null;
  const name = str(raw.name);
  if (name === null) return null;
  return {
    name,
    icao: str(raw.icao),
    iata: str(raw.iata),
    city: str(raw.city),
    country: str(raw.country),
    countryIso: str(raw.country_iso),
    lat: num(raw.lat),
    lon: num(raw.lon),
  };
}

function parseRoute(raw: unknown): OnlineRoute | null {
  if (!isRec(raw)) return null;
  const origin = airport(raw.origin);
  const destination = airport(raw.destination);
  if (origin === null || destination === null) return null;
  const al = isRec(raw.airline) ? raw.airline : null;
  return {
    origin,
    destination,
    midpoint: airport(raw.midpoint),
    callsign: str(raw.callsign),
    callsignIata: str(raw.callsign_iata),
    airline:
      al === null
        ? null
        : {
            name: str(al.name),
            icao: str(al.icao),
            iata: str(al.iata),
            country: str(al.country),
          },
  };
}

function parseAircraft(raw: unknown): OnlineAircraft | null {
  if (!isRec(raw)) return null;
  return {
    registration: str(raw.registration),
    type: str(raw.type),
    icaoType: str(raw.icao_type),
    manufacturer: str(raw.manufacturer),
    owner: str(raw.owner),
    ownerCountry: str(raw.owner_country),
    ownerCountryIso: str(raw.owner_country_iso),
  };
}

function parsePhoto(raw: unknown): OnlinePhoto | null {
  if (!isRec(raw)) return null;
  const src = str(raw.src);
  const link = str(raw.link);
  const photographer = str(raw.photographer);
  if (src === null || link === null || photographer === null) return null;
  if (!httpsOn(src, PHOTO_HOSTS) || !httpsOn(link, [PHOTO_PAGE_HOST])) return null;
  return { src, link, photographer, width: num(raw.width), height: num(raw.height) };
}

/** Parse the reply of `GET /api/v1/adsb/online`; null when it is not one. */
export function parseOnline(raw: unknown): OnlineReply | null {
  if (!isRec(raw) || typeof raw.enabled !== "boolean") return null;
  const sources = isRec(raw.sources) ? raw.sources : {};
  return {
    enabled: raw.enabled,
    locked: raw.locked === true,
    adsbdb: state(sources.adsbdb),
    planespotters: state(sources.planespotters),
    aircraft: parseAircraft(raw.aircraft),
    route: parseRoute(raw.route),
    photo: parsePhoto(raw.photo),
  };
}

export interface SeenCallsign {
  callsign: string;
  events: number;
  lastMs: number;
}

/** What the decoder history knows about an aircraft ("seen before"). */
export interface SeenHistory {
  events: number;
  /** Distinct days (UTC) with a history event. */
  days: number;
  firstMs: number;
  lastMs: number;
  callsigns: SeenCallsign[];
}

/** Parse the reply of `GET /api/v1/adsb/history`: null when the aircraft was never recorded. */
export function parseHistory(raw: unknown): SeenHistory | null {
  if (!isRec(raw) || raw.seen !== true) return null;
  const events = num(raw.events);
  const firstMs = num(raw.first_ms);
  const lastMs = num(raw.last_ms);
  if (events === null || firstMs === null || lastMs === null) return null;
  const callsigns: SeenCallsign[] = [];
  if (Array.isArray(raw.callsigns)) {
    for (const c of raw.callsigns) {
      if (!isRec(c)) continue;
      const callsign = str(c.callsign);
      const n = num(c.events);
      const last = num(c.last_ms);
      if (callsign !== null && n !== null && last !== null) {
        callsigns.push({ callsign, events: n, lastMs: last });
      }
    }
  }
  return { events, days: num(raw.days) ?? 1, firstMs, lastMs, callsigns };
}

/** The admin's switches (`GET`/`PUT /api/v1/admin/adsb/online`). */
export interface OnlineSettings {
  enabled: boolean;
  adsbdb: boolean;
  planespotters: boolean;
}

export interface OnlineSource {
  id: string;
  name: string;
  url: string;
  provides: string;
  sends: string;
}

export interface OnlineAdmin {
  settings: OnlineSettings;
  effective: OnlineSettings;
  locked: boolean;
  /** Name of the environment variable that overrides the switches. */
  env: string;
  sources: OnlineSource[];
}

const settings = (raw: unknown): OnlineSettings => {
  const r = isRec(raw) ? raw : {};
  return {
    enabled: r.enabled === true,
    adsbdb: r.adsbdb === true,
    planespotters: r.planespotters === true,
  };
};

export function parseOnlineAdmin(raw: unknown): OnlineAdmin | null {
  if (!isRec(raw) || !isRec(raw.settings) || !isRec(raw.effective)) return null;
  const sources: OnlineSource[] = [];
  if (Array.isArray(raw.sources)) {
    for (const s of raw.sources) {
      if (!isRec(s)) continue;
      const id = str(s.id);
      const name = str(s.name);
      const url = str(s.url);
      if (id === null || name === null || url === null) continue;
      sources.push({
        id,
        name,
        url,
        provides: str(s.provides) ?? "",
        sends: str(s.sends) ?? "",
      });
    }
  }
  return {
    settings: settings(raw.settings),
    effective: settings(raw.effective),
    locked: raw.locked === true,
    env: str(raw.env) ?? "SUPERHET_ADSB_ONLINE",
    sources,
  };
}

/** Is a source on and not switched off by the master switch. */
export const sourceOn = (s: OnlineSettings, id: "adsbdb" | "planespotters"): boolean =>
  s.enabled && s[id];
