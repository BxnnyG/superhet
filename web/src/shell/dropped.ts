// Dropped IQ blocks: the daemon counts blocks it had to throw away because a stage could not
// keep up (CPU too slow for the sample rate, USB hiccup). The count is cumulative since start.

import { useEffect, useRef, useState } from "react";
import type { DeviceState } from "../spectrum/api";

/** How long the top-bar chip stays after the count last grew. */
export const RECENT_MS = 60_000;

export interface DroppedNotice {
  /** Short label for the chip. */
  label: string;
  /** Total dropped blocks since the daemon started. */
  total: number;
  /** One-line summary. */
  summary: string;
  /** What to do. */
  action: string;
}

/** The notice for a device state, or null while nothing was ever dropped. */
export function droppedNotice(state: DeviceState | null): DroppedNotice | null {
  const total = state?.droppedBlocks ?? 0;
  if (total <= 0) return null;
  const n = total.toLocaleString("en-US");
  return {
    label: "Dropped",
    total,
    summary: `${n} IQ ${total === 1 ? "block was" : "blocks were"} dropped since the receiver started.`,
    action:
      "The processing could not keep up with the stream (audio and waterfall may glitch). Lower the sample rate or close other CPU-heavy programs; see docs/perf.md.",
  };
}

/**
 * True for a minute after the dropped-block count last grew while this page was open. A count
 * that is already non-zero when the page loads is history and does not light the chip.
 */
export function useRecentDrops(total: number | undefined): boolean {
  const [recent, setRecent] = useState(false);
  const last = useRef<number | undefined>(undefined);
  useEffect(() => {
    const prev = last.current;
    last.current = total;
    if (total === undefined || prev === undefined || total <= prev) return;
    setRecent(true);
    const timer = setTimeout(() => setRecent(false), RECENT_MS);
    return () => clearTimeout(timer);
  }, [total]);
  return recent;
}
