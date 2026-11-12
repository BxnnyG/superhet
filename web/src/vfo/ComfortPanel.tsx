import { useRef, type KeyboardEvent, type PointerEvent } from "react";
import { Button, Slider, cn } from "../ui";
import type { Vfo, VfoPatch } from "./api";
import {
  COMFORT_LABEL,
  NOTCH_FREQ_MAX_HZ,
  NOTCH_FREQ_MIN_HZ,
  NOTCH_WIDTH_MAX_HZ,
  NOTCH_WIDTH_MIN_HZ,
  audioRange,
  clampNotch,
  clampStrength,
  comfortSupported,
  isComfortOn,
  toggleComfort,
  type Comfort,
  type ComfortBlock,
  type NotchSetting,
} from "./comfort";

interface Props {
  vfo: Vfo;
  onUpdate: (id: number, patch: VfoPatch) => void;
}

const TOGGLES: { block: ComfortBlock; short: string; hint: string }[] = [
  {
    block: "nb",
    short: "Blanker",
    hint: "Cuts short crackles (ignition, power lines, lightning) before the channel filter",
  },
  {
    block: "notch",
    short: "Notch",
    hint: "Removes one tone at a frequency you pick; drag the marker over the passband",
  },
  {
    block: "autoNotch",
    short: "Auto notch",
    hint: "Finds and removes steady carriers and whistles by itself",
  },
  {
    block: "nr",
    short: "Noise red.",
    hint: "Lowers broadband hiss with spectral subtraction (about 8 ms delay)",
  },
];

const STRENGTH_LABEL: Record<Exclude<ComfortBlock, "notch">, string> = {
  nb: "Blanker strength",
  autoNotch: "Auto notch speed",
  nr: "Noise reduction",
};

/** Noise blanker, notches and noise reduction of the selected VFO (all off by default). */
export function ComfortPanel({ vfo, onUpdate }: Props) {
  if (!comfortSupported(vfo.mode)) {
    return (
      <p className="text-xs text-fg-muted">Noise tools are for AM, SSB, CW and NFM, not WFM.</p>
    );
  }
  const comfort: Comfort = vfo.comfort ?? {};
  const set = (next: Comfort) => onUpdate(vfo.id, { comfort: next });
  const strength = (block: Exclude<ComfortBlock, "notch">) => {
    const value = comfort[block];
    if (value === undefined) return null;
    return (
      <Slider
        key={block}
        label={STRENGTH_LABEL[block]}
        min={0}
        max={100}
        value={Math.round(value * 100)}
        valueLabel={`${Math.round(value * 100)} %`}
        onChange={(v) => set({ ...comfort, [block]: clampStrength(v / 100) })}
      />
    );
  };
  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-4">
        {TOGGLES.map(({ block, short, hint }) => {
          const on = isComfortOn(comfort, block);
          return (
            <Button
              key={block}
              size="sm"
              variant={on ? "primary" : "secondary"}
              aria-pressed={on}
              aria-label={COMFORT_LABEL[block]}
              title={on ? `${COMFORT_LABEL[block]} is on. Click to switch it off` : hint}
              onClick={() => set(toggleComfort(comfort, block))}
            >
              {short}
            </Button>
          );
        })}
      </div>
      {strength("nb")}
      {comfort.notch && (
        <div className="flex flex-col gap-2">
          <NotchStrip
            vfo={vfo}
            notch={comfort.notch}
            onChange={(notch) => set({ ...comfort, notch })}
          />
          <Slider
            label="Notch width"
            min={NOTCH_WIDTH_MIN_HZ}
            max={NOTCH_WIDTH_MAX_HZ}
            step={10}
            value={comfort.notch.widthHz}
            valueLabel={`${comfort.notch.widthHz} Hz`}
            onChange={(w) =>
              set({ ...comfort, notch: clampNotch({ ...comfort.notch!, widthHz: w }) })
            }
          />
        </div>
      )}
      {strength("autoNotch")}
      {strength("nr")}
    </div>
  );
}

/**
 * The audio passband of the VFO as a strip with a draggable notch marker. Drag, or focus it and
 * use the arrow keys (Shift = 100 Hz steps). The marker shows where in the audio the notch sits.
 */
export function NotchStrip({
  vfo,
  notch,
  onChange,
}: {
  vfo: Vfo;
  notch: NotchSetting;
  onChange: (n: NotchSetting) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const { loHz, hiHz } = audioRange(vfo);
  const span = Math.max(1, hiHz - loHz);
  const at = (hz: number) => Math.min(1, Math.max(0, (hz - loHz) / span));
  const fromPointer = (e: PointerEvent) => {
    const r = ref.current?.getBoundingClientRect();
    if (!r || r.width === 0) return;
    const f = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    onChange(clampNotch({ ...notch, freqHz: loHz + f * span }));
  };
  const onKey = (e: KeyboardEvent) => {
    const step = e.shiftKey ? 100 : 10;
    if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
      onChange(clampNotch({ ...notch, freqHz: notch.freqHz - step }));
    } else if (e.key === "ArrowRight" || e.key === "ArrowUp") {
      onChange(clampNotch({ ...notch, freqHz: notch.freqHz + step }));
    } else return;
    e.preventDefault();
  };
  const widthPct = Math.max(1.5, (notch.widthHz / span) * 100);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between text-xs text-fg-muted">
        <span>Notch frequency</span>
        <output className="font-mono text-fg num">{notch.freqHz} Hz</output>
      </div>
      <div
        ref={ref}
        data-testid="notch-strip"
        className="relative h-9 touch-none rounded-md bg-surface-2 ring-1 ring-border ring-inset"
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture?.(e.pointerId);
          fromPointer(e);
        }}
        onPointerMove={(e) => {
          if (e.buttons !== 0) fromPointer(e);
        }}
      >
        <div
          role="slider"
          tabIndex={0}
          aria-label="Notch frequency"
          aria-valuemin={NOTCH_FREQ_MIN_HZ}
          aria-valuemax={NOTCH_FREQ_MAX_HZ}
          aria-valuenow={notch.freqHz}
          aria-valuetext={`${notch.freqHz} Hz`}
          onKeyDown={onKey}
          className={cn(
            "absolute inset-y-0 -translate-x-1/2 cursor-ew-resize rounded-sm bg-danger/40 ring-1 ring-danger focus-visible:ring-2",
          )}
          style={{ left: `${at(notch.freqHz) * 100}%`, width: `${widthPct}%`, minWidth: 10 }}
        />
        <span className="pointer-events-none absolute bottom-0.5 left-1 font-mono text-[10px] text-fg-subtle">
          {Math.round(loHz)}
        </span>
        <span className="pointer-events-none absolute right-1 bottom-0.5 font-mono text-[10px] text-fg-subtle">
          {Math.round(hiHz)} Hz
        </span>
      </div>
    </div>
  );
}
