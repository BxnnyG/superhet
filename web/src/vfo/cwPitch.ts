// CW pitch (BFO offset): the tone a carrier on the VFO frequency is heard at. The daemon keeps
// the pitch per VFO (saved with the VFO state and profiles); the last pitch the user picked is
// also remembered in localStorage, so a VFO switched to CW later starts with the pitch the
// operator is used to instead of the 700 Hz default.

/** Pitch the daemon uses when none is set. */
export const CW_PITCH_DEFAULT_HZ = 700;
/** Range offered by the slider (the daemon accepts 200..2000 Hz). */
export const CW_PITCH_MIN_HZ = 400;
export const CW_PITCH_MAX_HZ = 1000;
export const CW_PITCH_STEP_HZ = 10;

const KEY = "superhet.cwPitch.v1";

function store(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** Clamp to the slider range and snap to its step. */
export function clampCwPitch(hz: number): number {
  if (!Number.isFinite(hz)) return CW_PITCH_DEFAULT_HZ;
  const snapped = Math.round(hz / CW_PITCH_STEP_HZ) * CW_PITCH_STEP_HZ;
  return Math.min(CW_PITCH_MAX_HZ, Math.max(CW_PITCH_MIN_HZ, snapped));
}

/** The remembered pitch, or null when the user never picked one (or it is the default). */
export function loadCwPitch(storage: Pick<Storage, "getItem"> | null = store()): number | null {
  try {
    const raw = storage?.getItem(KEY);
    if (!raw) return null;
    const hz = Number(raw);
    if (!Number.isInteger(hz) || hz < CW_PITCH_MIN_HZ || hz > CW_PITCH_MAX_HZ) return null;
    return hz === CW_PITCH_DEFAULT_HZ ? null : hz;
  } catch {
    return null;
  }
}

/** Remember the pitch the user picked; storage failures are ignored. */
export function saveCwPitch(hz: number, storage: Pick<Storage, "setItem"> | null = store()): void {
  try {
    storage?.setItem(KEY, String(clampCwPitch(hz)));
  } catch {
    // Private mode or quota: the per-VFO value on the daemon still works.
  }
}
