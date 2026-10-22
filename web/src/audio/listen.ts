// Per-device persistence of which VFOs are muted out of the mix ("not listening"). Storing the
// silent set (not the listening set) makes every new VFO listen by default.

const PREFIX = "superhet.listen.v1:";

function store(): Storage | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

export function listenKey(device: string): string {
  return PREFIX + device;
}

/** VFO ids that are not listened to; empty when nothing (valid) is stored. */
export function loadSilent(device: string, storage: Storage | null = store()): Set<number> {
  try {
    const raw = storage?.getItem(listenKey(device));
    if (!raw) return new Set();
    const parsed: unknown = JSON.parse(raw);
    const list = (parsed as { silent?: unknown } | null)?.silent;
    if (!Array.isArray(list)) return new Set();
    return new Set(list.filter((n): n is number => Number.isInteger(n) && n >= 1));
  } catch {
    return new Set();
  }
}

export function saveSilent(
  device: string,
  silent: ReadonlySet<number>,
  storage: Storage | null = store(),
): void {
  try {
    const key = listenKey(device);
    if (silent.size === 0) storage?.removeItem(key);
    else storage?.setItem(key, JSON.stringify({ silent: [...silent].sort((a, b) => a - b) }));
  } catch {
    // Storage may be full or blocked; listening then simply is not remembered.
  }
}
