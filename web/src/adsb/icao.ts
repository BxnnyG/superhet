// Facts derived from an aircraft's own data, without any lookup: the state that holds its ICAO
// address block (country and flag), the airline designator in its callsign, and the links out to photo sites (links only, nothing is fetched from them).

import { ICAO_RANGES } from "./icaoRanges";

export interface Country {
  /** ISO 3166-1 alpha-2 code, lower case; "" for blocks that belong to no state. */
  code: string;
  name: string;
}

/** The state an ICAO address block is allocated to; null for an unknown block or a `~` address. */
export function countryOf(icao: string): Country | null {
  if (!/^[0-9a-f]{6}$/i.test(icao)) return null;
  const value = parseInt(icao, 16);
  for (const [start, end, code, name] of ICAO_RANGES) {
    if (value >= start && value <= end) return { code, name };
  }
  return null;
}

/**
 * The flag as a regional-indicator emoji pair; "" without a state. Windows browsers draw these
 * as the two letters of the country code, which is why the code is also written out in the UI.
 */
export function flagEmoji(code: string): string {
  if (!/^[a-z]{2}$/i.test(code)) return "";
  const base = 0x1f1e6 - "A".charCodeAt(0);
  return [...code.toUpperCase()].map((c) => String.fromCodePoint(base + c.charCodeAt(0))).join("");
}

/**
 * ICAO airline designator of a callsign such as `DLH123` or `BAW9AB`: three letters followed by
 * a digit. Registrations used as callsigns (`DAIXA`, `N123AB`) and military callsigns without a
 * flight number have none.
 */
export function operatorCode(callsign: string | null): string | null {
  const match = callsign === null ? null : /^([A-Z]{3})[0-9]/.exec(callsign.trim().toUpperCase());
  return match?.[1] ?? null;
}

export interface PhotoLink {
  label: string;
  url: string;
}

/**
 * Links to look the aircraft up on photo sites. They only open when the user clicks them; the
 * page itself loads nothing from these sites. The ICAO address is the safest key (it is always
 * known); a registration, when the database has one, finds the aircraft on JetPhotos.
 */
export function photoLinks(icao: string, registration: string | null): PhotoLink[] {
  if (!/^[0-9a-f]{6}$/i.test(icao)) return [];
  const links: PhotoLink[] = [
    {
      label: "Planespotters",
      url: `https://www.planespotters.net/hex/${icao.toUpperCase()}`,
    },
  ];
  if (registration !== null && /^[A-Z0-9-]{2,12}$/i.test(registration)) {
    links.push({
      label: "JetPhotos",
      url: `https://www.jetphotos.com/registration/${encodeURIComponent(registration.toUpperCase())}`,
    });
  }
  return links;
}

/**
 * Links to follow the flight on public trackers: the ICAO address on ADS-B Exchange, an airline
 * callsign on FlightAware and Flightradar24. Plain links like `photoLinks`; nothing is fetched.
 */
export function trackerLinks(a: { icao: string; callsign: string | null }): PhotoLink[] {
  const links: PhotoLink[] = [];
  if (/^[0-9a-f]{6}$/i.test(a.icao)) {
    links.push({
      label: "ADS-B Exchange",
      url: `https://globe.adsbexchange.com/?icao=${a.icao.toLowerCase()}`,
    });
  }
  const callsign = a.callsign?.trim().toUpperCase() ?? "";
  if (/^[A-Z]{3}[0-9][0-9A-Z]{0,4}$/.test(callsign)) {
    links.push(
      { label: "FlightAware", url: `https://www.flightaware.com/live/flight/${callsign}` },
      { label: "Flightradar24", url: `https://www.flightradar24.com/${callsign}` },
    );
  }
  return links;
}
