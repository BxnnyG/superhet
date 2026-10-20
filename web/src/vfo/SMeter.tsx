import { useEffect, useRef } from "react";
import type { AudioController } from "../audio/controller";
import { Meter, type MeterTick } from "../ui";
import { cn } from "../ui/cn";
import { meterFraction } from "./model";

const REFRESH_MS = 100;
/** No frame for this long: the VFO is paused or the stream is lost. */
const STALE_MS = 1000;

const DBFS_TICKS: readonly MeterTick[] = [
  { at: 0, label: "-120" },
  { at: 0.25, label: "-90" },
  { at: 0.5, label: "-60" },
  { at: 0.75, label: "-30" },
  { at: 1, label: "0" },
];

/** S-meter from the audio frames' channel power; updates its DOM directly, outside React. */
export function SMeter({ audio, vfoId }: { audio: AudioController; vfoId: number }) {
  const barRef = useRef<HTMLDivElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const dotRef = useRef<HTMLSpanElement>(null);

  useEffect(() => {
    const update = () => {
      const m = audio.getMeter(vfoId);
      const live = m !== null && m.ageMs < STALE_MS;
      if (barRef.current) {
        barRef.current.style.width = `${(live ? meterFraction(m.powerDbfs) : 0) * 100}%`;
      }
      if (textRef.current) {
        textRef.current.textContent = live ? `${m.powerDbfs.toFixed(0)} dBFS` : "–";
      }
      if (dotRef.current) {
        dotRef.current.className = cn(
          "inline-block size-2 rounded-full transition-colors duration-150",
          live && m.squelchOpen ? "bg-success" : "bg-surface-3",
        );
      }
    };
    update();
    const timer = window.setInterval(update, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio, vfoId]);

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between">
        <span className="text-xs font-medium text-fg-muted">Signal</span>
        <span className="flex items-center gap-2">
          <span ref={dotRef} title="Squelch open" className="inline-block size-2 rounded-full" />
          <span ref={textRef} className="font-mono text-xs text-fg num" />
        </span>
      </div>
      <Meter barRef={barRef} aria-label="Signal strength" ticks={DBFS_TICKS} />
    </div>
  );
}
