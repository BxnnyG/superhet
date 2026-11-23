// What a transponder code (squawk) usually means. Codes are assigned by air traffic control, so
// most are meaningless to a bystander; only the codes below carry a standing meaning. National
// rules differ, which the text says where it matters.

export interface SquawkMeaning {
  /** One line for the panel. */
  text: string;
  /** 7500, 7600 and 7700: highlighted wherever they appear. */
  emergency: boolean;
}

const MEANINGS: Readonly<Record<string, SquawkMeaning>> = {
  "7500": { text: "Unlawful interference (hijack)", emergency: true },
  "7600": { text: "Radio failure (lost communications)", emergency: true },
  "7700": { text: "General emergency", emergency: true },
  "7000": {
    text: "VFR conspicuity code in most of Europe (no controller-assigned code)",
    emergency: false,
  },
  "1200": { text: "VFR conspicuity code in North America", emergency: false },
  "2000": {
    text: "Entering controlled airspace without an assigned code (ICAO)",
    emergency: false,
  },
  "1000": {
    text: "IFR flight in Mode S airspace (Europe, aircraft identified by callsign)",
    emergency: false,
  },
  "7004": { text: "Aerobatics or display flying (UK)", emergency: false },
  "7400": { text: "Unmanned aircraft lost its control link (UK and Europe)", emergency: false },
  "7777": { text: "Military intercept operation (not used by civil aircraft)", emergency: false },
  "0000": { text: "Unassigned or reserved code", emergency: false },
};

/** The standing meaning of a four-digit squawk code, or null when it has none. */
export function squawkMeaning(code: string | null): SquawkMeaning | null {
  return code === null ? null : (MEANINGS[code] ?? null);
}
