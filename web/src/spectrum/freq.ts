// Frequency entry parsing and display formatting (pure helpers).

const UNIT_FACTOR: Record<string, number> = {
  "": 1,
  hz: 1,
  k: 1e3,
  khz: 1e3,
  m: 1e6,
  mhz: 1e6,
  g: 1e9,
  ghz: 1e9,
};

/** Highest value (exclusive) that is read as MHz when no unit is given; larger means Hz. */
const UNITLESS_MHZ_LIMIT = 10_000;

/**
 * Parse user input into Hz. Accepts "104.2" (MHz), "433.92M", "7100k", "145500000" (Hz),
 * "2.4 GHz" and the display format "433.920 000". Returns null when unparsable or not positive.
 * Unitless values below 10000 are MHz, larger ones Hz.
 */
export function parseFrequency(input: string): number | null {
  const text = input
    .trim()
    .toLowerCase()
    .replace(/[\s_']/g, "")
    .replace(/,/g, ".");
  const m = /^(\d+\.?\d*|\.\d+)([a-z]*)$/.exec(text);
  if (!m) return null;
  const value = Number(m[1]);
  const unit = m[2] ?? "";
  if (!Number.isFinite(value)) return null;
  let factor = UNIT_FACTOR[unit];
  if (factor === undefined) return null;
  if (unit === "") factor = value < UNITLESS_MHZ_LIMIT ? 1e6 : 1;
  const hz = Math.round(value * factor);
  return hz > 0 && Number.isSafeInteger(hz) ? hz : null;
}

/** "433.920 000": MHz, kHz and Hz groups. */
export function formatFrequency(hz: number): string {
  const total = Math.max(0, Math.round(hz));
  const mhz = Math.floor(total / 1e6);
  const rest = total - mhz * 1e6;
  const khz = Math.floor(rest / 1e3);
  const h = rest - khz * 1e3;
  return `${mhz}.${String(khz).padStart(3, "0")} ${String(h).padStart(3, "0")}`;
}
