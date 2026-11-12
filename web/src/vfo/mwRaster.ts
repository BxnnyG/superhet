// Medium-wave channel raster: 9 kHz (ITU regions 1 and 3) or 10 kHz (region 2, the Americas).
// A browser preference in localStorage; without one the browser locale decides.

export type MwRaster = 9_000 | 10_000;

const KEY = "superhet.mwRaster.v1";
const listeners = new Set<() => void>();
let current: MwRaster | null = null;

/** Countries on the 10 kHz raster (ITU region 2 plus the Philippines and Korea's neighbours' habit). */
const TEN_KHZ_REGIONS = new Set([
  "US", "CA", "MX", "BR", "AR", "CL", "CO", "PE", "VE", "EC", "UY", "PY", "BO", "CR", "PA",
  "GT", "HN", "SV", "NI", "DO", "CU", "PR", "JM", "TT", "BS", "BB", "BZ", "GY", "SR", "HT", "PH",
]); // prettier-ignore

/** The raster a locale such as `en-US` implies (9 kHz when unknown). */
export function rasterForLocale(locale: string | undefined): MwRaster {
  const region = /[-_]([A-Za-z]{2})\b/.exec(locale ?? "")?.[1]?.toUpperCase();
  return region !== undefined && TEN_KHZ_REGIONS.has(region) ? 10_000 : 9_000;
}

function load(): MwRaster {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (raw === "9000") return 9_000;
    if (raw === "10000") return 10_000;
  } catch {
    // Fall through to the locale.
  }
  return rasterForLocale(typeof navigator === "undefined" ? undefined : navigator.language);
}

export function getMwRaster(): MwRaster {
  current ??= load();
  return current;
}

export function setMwRaster(raster: MwRaster): void {
  current = raster;
  try {
    window.localStorage.setItem(KEY, String(raster));
  } catch {
    // Not persisted.
  }
  for (const l of [...listeners]) l();
}

export function subscribeMwRaster(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Test hook: forget the cached value. */
export function resetMwRasterCache(): void {
  current = null;
}
