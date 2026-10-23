import type { AudioController } from "../audio/controller";
import { CTCSS_TONES, formatCtcss, type ToneState } from "../audio/tones";
import { Badge, Button, Select, Toggle, toasts, type SelectOption } from "../ui";
import type { Vfo, VfoPatch } from "./api";

interface Props {
  vfo: Vfo;
  /** Latest tone state of the VFO; undefined while nothing was detected. */
  tones: ToneState | undefined;
  audio: Pick<AudioController, "clearDigits">;
  onUpdate: (id: number, patch: VfoPatch) => void;
}

const SQUELCH_OPTIONS: readonly SelectOption[] = [
  { value: "off", label: "Off" },
  ...CTCSS_TONES.map((hz) => ({ value: String(hz), label: formatCtcss(hz) })),
];

async function copyDigits(digits: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(digits);
    toasts.push({ tone: "success", title: "Digits copied", key: "dtmf-copy", durationMs: 2000 });
  } catch {
    toasts.push({
      tone: "warn",
      title: "Could not copy",
      body: "The browser refused clipboard access.",
      key: "dtmf-copy",
    });
  }
}

/**
 * CTCSS sub-tone and DTMF digits of an NFM / AM VFO: the detected tone, an optional tone squelch
 * (the audio opens only while that tone is present) and the received digit string with copy and
 * clear. The row keeps its height whether or not anything was detected.
 */
export function TonesPanel({ vfo, tones, audio, onUpdate }: Props) {
  const enabled = vfo.tones === true;
  const squelchHz = vfo.toneSquelchHz ?? null;
  const detected = tones?.ctcssHz ?? null;
  const digits = tones?.digits ?? "";
  return (
    <div
      role="group"
      aria-label="Tones"
      className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2 px-3 py-2"
    >
      <span className="text-[11px] font-medium tracking-wide text-fg-subtle uppercase">Tones</span>
      <Toggle
        label="Detect CTCSS and DTMF"
        checked={enabled}
        onChange={(on) =>
          // A tone squelch cannot outlive the detector.
          onUpdate(vfo.id, on ? { tones: true } : { tones: false, toneSquelchHz: null })
        }
      />
      {enabled && (
        <>
          <div className="flex items-center gap-2" data-testid="ctcss">
            <span className="text-xs text-fg-muted">CTCSS</span>
            {detected !== null ? (
              <Badge tone="accent" title="Sub-tone currently received">
                {formatCtcss(detected)}
              </Badge>
            ) : (
              <span className="text-sm text-fg-subtle">none</span>
            )}
            <Select
              aria-label="Tone squelch"
              title="Tone squelch: the audio opens only while this sub-tone is received (about 0.6 s after the start of a transmission)"
              size="sm"
              className="w-28"
              options={SQUELCH_OPTIONS}
              value={squelchHz === null ? "off" : String(squelchHz)}
              onChange={(v) => onUpdate(vfo.id, { toneSquelchHz: v === "off" ? null : Number(v) })}
            />
            {detected !== null && detected !== squelchHz && (
              <Button
                size="sm"
                variant="ghost"
                title="Open the audio only for this sub-tone"
                onClick={() => onUpdate(vfo.id, { toneSquelchHz: detected })}
              >
                Use
              </Button>
            )}
          </div>
          <div className="flex min-w-0 flex-1 basis-56 items-center gap-2">
            <span className="text-xs text-fg-muted">DTMF</span>
            <span
              data-testid="dtmf-digits"
              title="Received DTMF digits"
              className="min-h-5 min-w-0 flex-1 truncate font-mono text-base tracking-widest text-fg"
            >
              {digits === "" ? <span className="text-fg-subtle">·</span> : digits}
            </span>
            <Button
              size="sm"
              variant="ghost"
              disabled={digits === ""}
              onClick={() => void copyDigits(digits)}
            >
              Copy
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={digits === ""}
              onClick={() => audio.clearDigits(vfo.id)}
            >
              Clear
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
