// The receiver position an operator may give when starting ADS-B (plugin config `lat` / `lon`):
// kept as text in the form and in localStorage, validated before it is sent.

export interface ReceiverText {
  lat: string;
  lon: string;
}

export const RECEIVER_STORAGE_KEY = "superhet.adsb.receiver.v1";

export function loadReceiverText(): ReceiverText {
  try {
    const raw: unknown = JSON.parse(window.localStorage.getItem(RECEIVER_STORAGE_KEY) ?? "null");
    if (raw !== null && typeof raw === "object") {
      const { lat, lon } = raw as Record<string, unknown>;
      if (typeof lat === "string" && typeof lon === "string") return { lat, lon };
    }
  } catch {
    // Unreadable or unavailable storage: start empty.
  }
  return { lat: "", lon: "" };
}

export function saveReceiverText(text: ReceiverText): void {
  try {
    if (text.lat.trim() === "" && text.lon.trim() === "") {
      window.localStorage.removeItem(RECEIVER_STORAGE_KEY);
    } else {
      window.localStorage.setItem(RECEIVER_STORAGE_KEY, JSON.stringify(text));
    }
  } catch {
    // Not persisted; the choice still applies now.
  }
}

export interface ParsedReceiver {
  /** Null when both fields are empty. */
  receiver: { lat: number; lon: number } | null;
  /** Why the input cannot be used; null when fine. */
  error: string | null;
}

const toNumber = (text: string): number =>
  text.trim() === "" ? NaN : Number(text.replace(",", "."));

export function parseReceiver(text: ReceiverText): ParsedReceiver {
  const empty = (s: string) => s.trim() === "";
  if (empty(text.lat) && empty(text.lon)) return { receiver: null, error: null };
  const lat = toNumber(text.lat);
  const lon = toNumber(text.lon);
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) {
    return { receiver: null, error: "Enter both latitude and longitude, or leave both empty." };
  }
  if (Math.abs(lat) > 90 || Math.abs(lon) > 180) {
    return { receiver: null, error: "Latitude is -90 to 90, longitude -180 to 180." };
  }
  return { receiver: { lat, lon }, error: null };
}
