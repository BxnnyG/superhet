// ACARS and VDL Mode 2 (org.superhet.decoder.acars / .vdl2, docs/plugins-acars.md): the plugins'
// event types, how their data is read for the log, and how a message is linked to the aircraft on
// the ADS-B map. The plugins are event-only; there is no status document.

import type { Aircraft } from "../adsb/model";
import type { AircraftInfo } from "../adsb/info";
import { isRec, type Rec } from "../spectrum/api";

export const ACARS_PLUGIN_ID = "org.superhet.decoder.acars";
export const VDL2_PLUGIN_ID = "org.superhet.decoder.vdl2";
export const ACARS_MESSAGE = "acars.message";
export const VDL2_MESSAGE = "vdl2.message";

export const isDatalinkKind = (kind: string): boolean =>
  kind === ACARS_MESSAGE || kind === VDL2_MESSAGE;

/** What the label of an ACARS message says, for the labels that are the same everywhere. */
const LABELS: Record<string, string> = {
  Q0: "Link test",
  SA: "Media advisory",
  H1: "Message to or from a terminal",
  "5Z": "Airline designated downlink",
  "5U": "Weather request",
  _d: "Demand mode response",
};

export function labelText(label: string | null): string | null {
  return label === null ? null : (LABELS[label] ?? null);
}

/** Which aircraft a message is about: the 24-bit address (VDL2) and/or the registration. */
export interface AircraftRef {
  icao: string | null;
  reg: string | null;
}

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

/** The aircraft a datalink event names, or null if it names none. */
export function aircraftRef(d: Rec): AircraftRef | null {
  const icao = str(d.icao);
  const reg = str(d.tail);
  const valid = icao !== null && /^[0-9a-f]{6}$/i.test(icao) ? icao.toLowerCase() : null;
  return valid === null && reg === null ? null : { icao: valid, reg };
}

/** A registration written as `D-AIPA`, `DAIPA` or ` .D-AIPA`, compared without dashes and case. */
export function normalizeReg(reg: string): string {
  return reg.replace(/[^0-9a-z]/gi, "").toUpperCase();
}

/**
 * The aircraft of the ADS-B picture a message is about: by ICAO address, else by registration
 * (known from the imported aircraft database only).
 */
export function findAircraft(
  aircraft: readonly Aircraft[],
  info: (icao: string) => AircraftInfo | null,
  ref: AircraftRef,
): Aircraft | null {
  if (ref.icao !== null) {
    const hit = aircraft.find((a) => a.icao === ref.icao);
    if (hit !== undefined) return hit;
  }
  if (ref.reg !== null) {
    const wanted = normalizeReg(ref.reg);
    if (wanted === "") return null;
    for (const a of aircraft) {
      const reg = info(a.icao)?.registration ?? null;
      if (reg !== null && normalizeReg(reg) === wanted) return a;
    }
  }
  return null;
}

/** The decoded ARINC 622 / X.25 content of a message as indented JSON; null if there is none. */
export function decodedJson(d: Rec): string | null {
  const decoded = d.decoded;
  if (!isRec(decoded) || Object.keys(decoded).length === 0) return null;
  return JSON.stringify(decoded, null, 2);
}
