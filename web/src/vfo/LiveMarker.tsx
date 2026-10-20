import { useEffect, useRef } from "react";
import type { AudioController } from "../audio/controller";
import { markerLeft } from "../ui";
import { SQUELCH_OFF_POS } from "./model";

const REFRESH_MS = 100;
const STALE_MS = 1000;

/** Slider-position fraction (0..1) of a channel power on the squelch slider scale. */
export function squelchFraction(powerDbfs: number): number {
  return Math.min(1, Math.max(0, (powerDbfs - SQUELCH_OFF_POS) / -SQUELCH_OFF_POS));
}

/** Live channel-power tick drawn on the squelch slider track (DOM updated outside React). */
export function LiveMarker({ audio, vfoId }: { audio: AudioController; vfoId: number }) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const update = () => {
      const el = ref.current;
      if (!el) return;
      const m = audio.getMeter(vfoId);
      const live = m !== null && m.ageMs < STALE_MS;
      el.style.opacity = live ? "1" : "0";
      if (live) el.style.left = markerLeft(squelchFraction(m.powerDbfs));
    };
    update();
    const timer = window.setInterval(update, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio, vfoId]);
  return (
    <span
      ref={ref}
      aria-hidden="true"
      title="Live channel power"
      className="pointer-events-none absolute top-1/2 h-3 w-0.5 -translate-x-1/2 -translate-y-1/2 rounded-full bg-success opacity-0 transition-opacity duration-150"
    />
  );
}
