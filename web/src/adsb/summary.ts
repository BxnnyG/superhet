// The text "Copy" and "Share" put on the clipboard: what is known about one aircraft, readable on
// its own. It never contains the distance or bearing from the receiver (together with the
// aircraft's position they would give the receiver's location away) or anything about this server.

import { countryOf } from "./icao";
import { formatAlt, formatSpeed, label, type Tracked } from "./model";
import type { OnlineReply } from "./online";
import type { AircraftInfo, OperatorInfo } from "./info";
import { squawkMeaning } from "./squawk";
import type { UnitSystem } from "./units";

export interface SummaryInput {
  a: Tracked;
  units: UnitSystem;
  known: AircraftInfo | null;
  operator: OperatorInfo | null;
  online: OnlineReply | null;
}

/** Full type name: the online one when there is one, else what the local database says. */
export function typeName(known: AircraftInfo | null, online: OnlineReply | null): string | null {
  const on = online?.aircraft;
  if (on?.type != null) {
    const maker = on.manufacturer ?? "";
    const name = on.type.toLowerCase().startsWith(maker.toLowerCase())
      ? on.type
      : `${maker} ${on.type}`;
    return name.trim();
  }
  if (known === null) return null;
  const local = [known.typeCode, known.typeDesc].filter(Boolean).join(" ");
  return local === "" ? null : local;
}

/** One line per fact, only the facts that are known. */
export function summaryText({ a, units, known, operator, online }: SummaryInput): string {
  const lines: string[] = [];
  lines.push(`${label(a)} (ICAO ${a.icao.toUpperCase()})`);
  const registration = known?.registration ?? online?.aircraft?.registration ?? null;
  const type = typeName(known, online);
  if (registration !== null || type !== null) {
    lines.push([registration, type].filter(Boolean).join(", "));
  }
  const owner = operator?.name ?? online?.route?.airline?.name ?? online?.aircraft?.owner ?? null;
  if (owner !== null) lines.push(`Operator: ${owner}`);
  const country = countryOf(a.icao);
  if (country !== null) lines.push(`Registered in: ${country.name}`);
  const route = online?.route;
  if (route !== null && route !== undefined) {
    const end = (x: { iata: string | null; icao: string | null; name: string }): string =>
      x.iata ?? x.icao ?? x.name;
    lines.push(`Route (database, may be wrong): ${end(route.origin)} to ${end(route.destination)}`);
  }
  const alt = formatAlt(a, units).text;
  const speed = formatSpeed(a.speedKt, units).text;
  const now = [alt, speed].filter((t) => t !== "–");
  if (now.length > 0) lines.push(now.join(", "));
  if (a.squawk !== null) {
    const meaning = squawkMeaning(a.squawk);
    lines.push(`Squawk ${a.squawk}${meaning === null ? "" : ` (${meaning.text})`}`);
  }
  if (a.lat !== null && a.lon !== null) {
    lines.push(`Position: ${a.lat.toFixed(4)}, ${a.lon.toFixed(4)}`);
  }
  if (/^[0-9a-f]{6}$/i.test(a.icao)) {
    lines.push(`https://globe.adsbexchange.com/?icao=${a.icao.toLowerCase()}`);
  }
  return lines.join("\n");
}
