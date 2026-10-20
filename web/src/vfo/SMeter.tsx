import { useEffect, useRef } from "react";
import type { AudioController } from "../audio/controller";
import { meterFraction } from "./model";

const REFRESH_MS = 100;
/** No frame for this long: the VFO is paused or the stream is lost. */
const STALE_MS = 1000;

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
        dotRef.current.className = `inline-block size-2 rounded-full ${
          live && m.squelchOpen ? "bg-emerald-400" : "bg-neutral-600"
        }`;
      }
    };
    update();
    const timer = window.setInterval(update, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio, vfoId]);

  return (
    <div className="flex items-center gap-2" role="meter" aria-label="Signal strength">
      <span ref={dotRef} title="Squelch open" className="inline-block size-2 rounded-full" />
      <div className="h-2.5 w-32 overflow-hidden rounded bg-neutral-800">
        <div ref={barRef} className="h-full bg-sky-400" style={{ width: "0%" }} />
      </div>
      <span ref={textRef} className="w-16 font-mono text-xs tabular-nums text-neutral-400" />
    </div>
  );
}
