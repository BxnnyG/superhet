// "Mouse wheel: zoom | tune" preference, kept in localStorage. A tiny external store so the
// settings menu (React) and the spectrum overlay (imperative) read the same value.

import type { WheelMode } from "./gestures";

const KEY = "superhet.wheelMode";
const listeners = new Set<() => void>();
let current: WheelMode | null = null;

function load(): WheelMode {
  try {
    return window.localStorage.getItem(KEY) === "tune" ? "tune" : "zoom";
  } catch {
    return "zoom";
  }
}

export function getWheelMode(): WheelMode {
  current ??= load();
  return current;
}

export function setWheelMode(mode: WheelMode): void {
  current = mode;
  try {
    window.localStorage.setItem(KEY, mode);
  } catch {
    // Not persisted.
  }
  for (const l of [...listeners]) l();
}

export function subscribeWheelMode(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Test hook: forget the cached value so the next read goes to localStorage again. */
export function resetWheelModeCache(): void {
  current = null;
}
