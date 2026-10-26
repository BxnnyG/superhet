// Master output volume (0..1, applied on top of every VFO's own volume). Persisted per browser.

export const MASTER_STORAGE_KEY = "superhet.audio.master.v1";
export const DEFAULT_MASTER = 1;

function store(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function clampMaster(v: number): number {
  return Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : DEFAULT_MASTER;
}

export function loadMaster(storage: Storage | null = store()): number {
  try {
    const raw = storage?.getItem(MASTER_STORAGE_KEY);
    if (raw == null || raw.trim() === "") return DEFAULT_MASTER;
    const n = Number(raw);
    return Number.isFinite(n) ? clampMaster(n) : DEFAULT_MASTER;
  } catch {
    return DEFAULT_MASTER;
  }
}

export function saveMaster(v: number, storage: Storage | null = store()): void {
  try {
    if (v === DEFAULT_MASTER) storage?.removeItem(MASTER_STORAGE_KEY);
    else storage?.setItem(MASTER_STORAGE_KEY, String(v));
  } catch {
    // Not persisted; the level still applies for this session.
  }
}
