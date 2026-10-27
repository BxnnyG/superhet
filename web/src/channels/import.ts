// Importing a custom plan from a file: a channel plan JSON document (docs/channel-plans.md) or a
// CHIRP CSV export (name, frequency, mode, tone become the channels of a new plan).

import { CTCSS_TONES } from "../audio/tones";
import { parseChirp } from "../bookmarks/formats";
import { parsePlan, PlanError, type Plan } from "./schema";

/** Plan name from a file name: "club list.csv" -> "club list". */
export function nameFromFile(fileName: string): string {
  const base = fileName.replace(/\.[^.]+$/, "").trim();
  return (base === "" ? "Imported channels" : base).slice(0, 100);
}

/**
 * Parse an imported file into a plan (not yet stored). `region` is the user's region, used for
 * CHIRP lists that carry none. Throws a `PlanError` with the reason.
 */
export function planFromFile(text: string, fileName: string, region: string | null): Plan {
  const trimmed = text.trim();
  if (trimmed === "") throw new PlanError("the file is empty");
  if (trimmed.startsWith("{")) {
    let raw: unknown;
    try {
      raw = JSON.parse(trimmed);
    } catch (e) {
      throw new PlanError(`not valid JSON (${e instanceof Error ? e.message : "parse error"})`);
    }
    const plan = parsePlan(raw, { id: "custom:import" });
    return plan;
  }
  const result = parseChirp(trimmed);
  if (result.bookmarks.length === 0) {
    throw new PlanError("no channels found (expected a plan JSON or a CHIRP CSV)");
  }
  const channels = result.bookmarks.map((b, i) => ({
    number: String(i + 1),
    ...(b.name !== "" ? { name: b.name } : {}),
    freq_hz: Math.round(b.freqHz),
    ...(b.ctcssHz !== null && CTCSS_TONES.includes(b.ctcssHz) ? { ctcss_hz: b.ctcssHz } : {}),
    ...(b.mode ? { mode: b.mode } : {}),
    ...(b.bandwidthHz ? { bandwidth_hz: b.bandwidthHz } : {}),
    ...(b.notes ? { note: b.notes } : {}),
  }));
  const name = nameFromFile(fileName);
  return parsePlan(
    {
      v: 1,
      name,
      service: "Custom",
      chip: "Ch",
      regions: region ? [region] : ["WORLD"],
      licence: "licensed",
      licence_note: "Imported from CHIRP; check which of these you may transmit on.",
      mode: "nfm",
      source: `CHIRP CSV ${fileName}`,
      channels,
    },
    { id: "custom:import" },
  );
}
