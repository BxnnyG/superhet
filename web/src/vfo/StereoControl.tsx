import { useEffect, useRef } from "react";
import type { AudioController } from "../audio/controller";
import { Button, cn } from "../ui";
import type { Vfo, VfoPatch } from "./api";

const REFRESH_MS = 200;
const STALE_MS = 1000;

interface Props {
  vfo: Vfo;
  audio: AudioController;
  onUpdate: (id: number, patch: VfoPatch) => void;
}

/**
 * Stereo switch of a WFM VFO plus the "ST" lamp: lit while the audio really is stereo, i.e. the
 * transmitter's pilot is locked. With stereo off (or a mono station) the VFO plays mono.
 */
export function StereoControl({ vfo, audio, onUpdate }: Props) {
  const on = vfo.stereo !== false;
  const lampRef = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const update = () => {
      const el = lampRef.current;
      if (!el) return;
      const m = audio.getMeter(vfo.id);
      const lit = on && m !== null && m.ageMs < STALE_MS && m.stereo;
      el.dataset.lit = lit ? "true" : "false";
      el.className = cn(
        "inline-flex h-5 items-center rounded px-1.5 text-[11px] font-semibold tracking-wide ring-1 ring-inset transition-colors duration-150",
        lit
          ? "bg-success-soft text-success ring-success/30"
          : "bg-surface-2 text-fg-subtle ring-border",
      );
    };
    update();
    const timer = window.setInterval(update, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [audio, vfo.id, on]);

  return (
    <div className="flex items-center gap-1.5">
      <Button
        size="sm"
        variant={on ? "primary" : "secondary"}
        aria-pressed={on}
        title={
          on
            ? "Stereo decoding is on. Click to listen in mono"
            : "Mono. Click to decode stereo when the station sends it"
        }
        onClick={() => onUpdate(vfo.id, { stereo: !on })}
      >
        Stereo
      </Button>
      <span
        ref={lampRef}
        data-testid="stereo-lamp"
        title="Lit while a stereo pilot is received"
        className="inline-flex h-5 items-center rounded bg-surface-2 px-1.5 text-[11px] font-semibold text-fg-subtle"
      >
        ST
      </span>
    </div>
  );
}
