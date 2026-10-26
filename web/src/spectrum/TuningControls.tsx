import { usePermissions } from "../auth/context";
import { NEEDS_OPERATOR } from "../auth/permissions";
import { lockedBy } from "../state/locks";
import { droppedNotice } from "../shell/dropped";
import { overloadNotice } from "../shell/overload";
import { Alert, LockIcon, NumberField, Select, Slider, Toggle } from "../ui";
import type { TunePatch } from "./api";
import { formatFrequency, parseFrequency } from "./freq";
import type { TuningSnapshot } from "./tuning";

/** Sample rates offered for RTL-SDR dongles (S/s). */
export const SAMPLE_RATES = [
  250_000, 1_024_000, 1_400_000, 1_800_000, 2_048_000, 2_400_000, 2_560_000, 2_880_000, 3_200_000,
];

export function formatRate(rate: number): string {
  return `${parseFloat((rate / 1e6).toFixed(3))} MS/s`;
}

/** Index of the entry in `gains` closest to `db` (0 for an empty list). */
export function nearestGainIndex(gains: readonly number[], db: number | null): number {
  if (db === null || gains.length === 0) return 0;
  let best = 0;
  for (let i = 1; i < gains.length; i++) {
    if (Math.abs((gains[i] ?? 0) - db) < Math.abs((gains[best] ?? 0) - db)) best = i;
  }
  return best;
}

interface Props {
  tuning: TuningSnapshot;
  onTune: (patch: TunePatch) => void;
}

/** Source section content: centre frequency, sample rate, gain, PPM. */
export function TuningControls({ tuning, onTune }: Props) {
  const { display, busy } = tuning;
  // Centre, rate, gain and ppm need the operator role (POST /tune); others see the values only.
  const readOnly = !usePermissions().canTune;
  const holders = tuning.state?.locks ?? [];
  const locked = holders.length > 0;
  const rates =
    display && !SAMPLE_RATES.includes(display.sampleRate)
      ? [...SAMPLE_RATES, display.sampleRate].sort((a, b) => a - b)
      : SAMPLE_RATES;
  const gain = display?.gain ?? null;
  const gains = display?.gainsDb ?? [];
  const gainIdx = nearestGainIndex(gains, gain?.db ?? null);
  const gainDb = gains[gainIdx];
  const overload = overloadNotice(tuning.state);
  const dropped = droppedNotice(tuning.state);

  return (
    <div className="flex flex-col gap-3" role="group" aria-label="Source">
      <NumberField
        label="Centre frequency"
        unit="MHz"
        min={1}
        step={1000}
        value={display?.centerHz ?? 0}
        format={formatFrequency}
        parse={parseFrequency}
        disabled={!display || locked || readOnly}
        onCommit={(hz) => onTune({ centerHz: hz })}
      />
      <Select
        label="Sample rate"
        disabled={!display || busy || locked || readOnly}
        value={display ? String(display.sampleRate) : ""}
        onChange={(v) => onTune({ sampleRate: Number(v) })}
        options={[
          ...(display ? [] : [{ value: "", label: "–" }]),
          ...rates.map((r) => ({ value: String(r), label: formatRate(r) })),
        ]}
      />
      {readOnly && (
        <p role="note" className="text-xs text-fg-subtle">
          {NEEDS_OPERATOR} can change the source settings.
        </p>
      )}
      {locked && !readOnly && (
        <p role="status" className="flex items-start gap-1.5 text-xs text-warn">
          <LockIcon size={14} className="mt-px shrink-0" />
          <span>{lockedBy(holders)}. Unlock to change the centre frequency or sample rate.</span>
        </p>
      )}
      {overload && (
        <Alert tone="warn" className="text-xs">
          <p className="font-medium">{overload.summary}</p>
          <p className="mt-0.5">{overload.action}</p>
        </Alert>
      )}
      {dropped && (
        <div data-testid="dropped-notice">
          <Alert tone="info" className="text-xs">
            <p className="font-medium">{dropped.summary}</p>
            <p className="mt-0.5">{dropped.action}</p>
          </Alert>
        </div>
      )}
      {gain === null ? (
        display ? (
          <p className="text-xs text-fg-subtle">This device has no gain control.</p>
        ) : null
      ) : (
        <div className="flex flex-col gap-2">
          <Toggle
            label="Automatic gain"
            checked={gain.mode === "auto"}
            disabled={busy || readOnly}
            onChange={(auto) => onTune({ gain: auto ? "auto" : (gainDb ?? 0) })}
          />
          <Slider
            label="Gain"
            min={0}
            max={Math.max(0, gains.length - 1)}
            value={gainIdx}
            disabled={gain.mode === "auto" || gains.length < 2 || readOnly}
            valueLabel={gain.mode === "auto" ? "auto" : `${(gainDb ?? 0).toFixed(1)} dB`}
            onChange={(i) => {
              const db = gains[i];
              if (db !== undefined) onTune({ gain: db });
            }}
          />
        </div>
      )}
      <NumberField
        label="Frequency correction"
        unit="ppm"
        min={-1000}
        max={1000}
        value={display?.ppm ?? 0}
        disabled={!display || readOnly}
        onCommit={(ppm) => onTune({ ppm })}
      />
    </div>
  );
}
