// Weather satellites: wire types of the API (docs/api-weather.md), parsing and display helpers.

import { formatMhz } from "../format";

export const WEATHER_PLUGIN_ID = "org.superhet.decoder.weather";
export const WEATHER_PASS = "weather.pass";
export const WEATHER_RECORDING = "weather.recording";
export const WEATHER_FAILED = "weather.failed";

export type SatelliteStatus = "operational" | "decommissioned" | "no_images";

export interface Downlink {
  hz: number;
  label: string;
}

export interface Satellite {
  noradId: number;
  name: string;
  format: string;
  status: SatelliteStatus;
  note: string;
  downlinks: Downlink[];
}

export interface ElementsInfo {
  enabled: boolean;
  source: "download" | "manual" | null;
  fetchedMs: number | null;
  attemptMs: number | null;
  error: string | null;
  /** Seconds until the next download is allowed. */
  waitS: number;
  satellites: { noradId: number; name: string | null; epochMs: number }[];
  download: { name: string; url: string; infoUrl: string; minIntervalS: number };
}

export interface WeatherConfig {
  satellites: Satellite[];
  checked: string;
  elements: ElementsInfo;
  store: { available: boolean; captures: number; usedBytes: number; maxBytes: number };
}

export interface PassPrediction {
  satellite: string;
  noradId: number;
  format: string;
  aosMs: number;
  tcaMs: number;
  losMs: number;
  maxElevationDeg: number;
  aosAzimuthDeg: number;
  tcaAzimuthDeg: number;
  losAzimuthDeg: number;
  inProgress: boolean;
  downlinks: Downlink[];
  elementsAgeH: number;
}

export interface PassesReply {
  generatedMs: number;
  positionFrom: "request" | "adsb";
  elements: { source: string | null; fetchedMs: number | null };
  passes: PassPrediction[];
}

export interface CaptureImage {
  name: string;
  label: string;
  width: number;
  height: number;
  size: number;
  thumb: string | null;
}

export interface Capture {
  id: string;
  satellite: string | null;
  pipeline: "lrpt" | "apt";
  startedMs: number;
  durationS: number;
  frequencyHz: number | null;
  device: string | null;
  maxElevationDeg: number | null;
  peakDb: number | null;
  keep: boolean;
  sizeBytes: number;
  images: CaptureImage[];
}

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

function parseDownlink(raw: unknown): Downlink | null {
  if (!isRec(raw)) return null;
  const hz = num(raw.hz);
  return hz === null ? null : { hz, label: str(raw.label) ?? formatMhz(hz) };
}

const STATUSES: readonly string[] = ["operational", "decommissioned", "no_images"];

function parseSatellite(raw: unknown): Satellite | null {
  if (!isRec(raw)) return null;
  const noradId = num(raw.norad_id);
  const name = str(raw.name);
  const status = str(raw.status);
  if (noradId === null || name === null || status === null || !STATUSES.includes(status)) {
    return null;
  }
  return {
    noradId,
    name,
    format: str(raw.format) ?? "",
    status: status as SatelliteStatus,
    note: str(raw.note) ?? "",
    downlinks: list(raw.downlinks)
      .map(parseDownlink)
      .filter((d): d is Downlink => d !== null),
  };
}

export function parseElements(raw: unknown): ElementsInfo {
  const r = isRec(raw) ? raw : {};
  const dl = isRec(r.download) ? r.download : {};
  return {
    enabled: r.enabled === true,
    source: r.source === "download" || r.source === "manual" ? r.source : null,
    fetchedMs: num(r.fetched_ms),
    attemptMs: num(r.attempt_ms),
    error: str(r.error),
    waitS: num(r.wait_s) ?? 0,
    satellites: list(r.satellites).flatMap((s) => {
      if (!isRec(s)) return [];
      const noradId = num(s.norad_id);
      const epochMs = num(s.epoch_ms);
      return noradId === null || epochMs === null ? [] : [{ noradId, name: str(s.name), epochMs }];
    }),
    download: {
      name: str(dl.name) ?? "CelesTrak",
      url: str(dl.url) ?? "",
      infoUrl: str(dl.info_url) ?? "",
      minIntervalS: num(dl.min_interval_s) ?? 7200,
    },
  };
}

export function parseConfig(raw: unknown): WeatherConfig {
  const r = isRec(raw) ? raw : {};
  const store = isRec(r.store) ? r.store : {};
  return {
    satellites: list(r.satellites)
      .map(parseSatellite)
      .filter((s): s is Satellite => s !== null),
    checked: str(r.checked) ?? "",
    elements: parseElements(r.elements),
    store: {
      available: store.available === true,
      captures: num(store.captures) ?? 0,
      usedBytes: num(store.used_bytes) ?? 0,
      maxBytes: num(store.max_bytes) ?? 0,
    },
  };
}

export function parsePass(raw: unknown): PassPrediction | null {
  if (!isRec(raw)) return null;
  const satellite = str(raw.satellite);
  const aos = num(raw.aos_ms);
  const tca = num(raw.tca_ms);
  const los = num(raw.los_ms);
  const max = num(raw.max_elevation_deg);
  if (satellite === null || aos === null || tca === null || los === null || max === null) {
    return null;
  }
  return {
    satellite,
    noradId: num(raw.norad_id) ?? 0,
    format: str(raw.format) ?? "",
    aosMs: aos,
    tcaMs: tca,
    losMs: los,
    maxElevationDeg: max,
    aosAzimuthDeg: num(raw.aos_azimuth_deg) ?? 0,
    tcaAzimuthDeg: num(raw.tca_azimuth_deg) ?? 0,
    losAzimuthDeg: num(raw.los_azimuth_deg) ?? 0,
    inProgress: raw.in_progress === true,
    downlinks: list(raw.downlinks)
      .map(parseDownlink)
      .filter((d): d is Downlink => d !== null),
    elementsAgeH: num(raw.elements_age_h) ?? 0,
  };
}

export function parsePasses(raw: unknown): PassesReply {
  const r = isRec(raw) ? raw : {};
  const el = isRec(r.elements) ? r.elements : {};
  return {
    generatedMs: num(r.generated_ms) ?? Date.now(),
    positionFrom: r.position_from === "adsb" ? "adsb" : "request",
    elements: { source: str(el.source), fetchedMs: num(el.fetched_ms) },
    passes: list(r.passes)
      .map(parsePass)
      .filter((p): p is PassPrediction => p !== null),
  };
}

export function parseCapture(raw: unknown): Capture | null {
  if (!isRec(raw)) return null;
  const id = str(raw.id);
  const started = num(raw.started_ms);
  if (id === null || started === null) return null;
  return {
    id,
    satellite: str(raw.satellite),
    pipeline: raw.pipeline === "apt" ? "apt" : "lrpt",
    startedMs: started,
    durationS: num(raw.duration_s) ?? 0,
    frequencyHz: num(raw.frequency_hz),
    device: str(raw.device),
    maxElevationDeg: num(raw.max_elevation_deg),
    peakDb: num(raw.peak_db),
    keep: raw.keep === true,
    sizeBytes: num(raw.size_bytes) ?? 0,
    images: list(raw.images).flatMap((i) => {
      if (!isRec(i)) return [];
      const name = str(i.name);
      return name === null
        ? []
        : [
            {
              name,
              label: str(i.label) ?? name,
              width: num(i.width) ?? 0,
              height: num(i.height) ?? 0,
              size: num(i.size) ?? 0,
              thumb: str(i.thumb),
            },
          ];
    }),
  };
}

// ---- display ----

/** `in 12 min`, `in 2 h 05 min`, `overhead now` while the satellite is up, `passed` afterwards. */
export function countdown(pass: Pick<PassPrediction, "aosMs" | "losMs">, now: number): string {
  if (now >= pass.losMs) return "passed";
  if (now >= pass.aosMs) return "overhead now";
  const s = Math.round((pass.aosMs - now) / 1000);
  if (s < 60) return `in ${s} s`;
  const m = Math.round(s / 60);
  if (m < 60) return `in ${m} min`;
  const h = Math.floor(m / 60);
  const rest = m % 60;
  return rest === 0 ? `in ${h} h` : `in ${h} h ${String(rest).padStart(2, "0")} min`;
}

const POINTS = [
  "N",
  "NNE",
  "NE",
  "ENE",
  "E",
  "ESE",
  "SE",
  "SSE",
  "S",
  "SSW",
  "SW",
  "WSW",
  "W",
  "WNW",
  "NW",
  "NNW",
] as const;

/** Compass point of an azimuth in degrees (`59` -> `ENE`). */
export function compass(deg: number): string {
  const i = Math.round((((deg % 360) + 360) % 360) / 22.5) % 16;
  return POINTS[i] ?? "N";
}

/** Local `HH:MM`. */
export function clock(ms: number): string {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** Local weekday and date for a pass list, `null` for today. */
export function dayLabel(ms: number, now: number): string | null {
  const d = new Date(ms);
  if (d.toDateString() === new Date(now).toDateString()) return null;
  return d.toLocaleDateString("en-GB", { weekday: "short", day: "numeric", month: "short" });
}

/** How good a pass is for reception: low passes are weak, high ones strong. */
export function quality(maxElevationDeg: number): "poor" | "fair" | "good" | "excellent" {
  if (maxElevationDeg >= 50) return "excellent";
  if (maxElevationDeg >= 30) return "good";
  if (maxElevationDeg >= 15) return "fair";
  return "poor";
}

/** Primary downlink of a pass (the first one the catalogue lists). */
export function primaryHz(pass: Pick<PassPrediction, "downlinks">): number | null {
  return pass.downlinks[0]?.hz ?? null;
}

export function imageUrl(
  id: string,
  name: string,
  opts: { thumb?: boolean; download?: boolean } = {},
): string {
  const q = new URLSearchParams();
  if (opts.thumb === true) q.set("thumb", "true");
  if (opts.download === true) q.set("download", "true");
  const qs = q.toString();
  return `/api/v1/weather/captures/${encodeURIComponent(id)}/images/${encodeURIComponent(name)}${qs === "" ? "" : `?${qs}`}`;
}

/** Hours since an element set's epoch, as `3 h` / `2 d`. */
export function ageText(epochMs: number, now: number): string {
  const h = Math.max(0, (now - epochMs) / 3_600_000);
  return h < 48 ? `${Math.round(h)} h` : `${Math.round(h / 24)} d`;
}
