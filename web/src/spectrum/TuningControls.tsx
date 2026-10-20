import { useState } from "react";
import { formatFrequency, parseFrequency } from "./freq";
import type { TuningSnapshot } from "./tuning";
import type { TunePatch } from "./api";

/** Sample rates offered for RTL-SDR dongles (S/s). */
export const SAMPLE_RATES = [
  250_000, 1_024_000, 1_400_000, 1_800_000, 2_048_000, 2_400_000, 2_560_000, 2_880_000, 3_200_000,
];

export function formatRate(rate: number): string {
  return `${parseFloat((rate / 1e6).toFixed(3))} MS/s`;
}

interface Props {
  tuning: TuningSnapshot;
  onTune: (patch: TunePatch) => void;
}

const selectClass =
  "rounded border border-neutral-600 bg-neutral-900 px-1 py-0.5 disabled:opacity-40";

export function FrequencyInput({
  centerHz,
  onCommit,
}: {
  centerHz: number | null;
  onCommit: (hz: number) => void;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(false);
  const shown = draft ?? (centerHz === null ? "" : formatFrequency(centerHz));

  const cancel = () => {
    setDraft(null);
    setInvalid(false);
  };

  return (
    <label className="flex items-center gap-1">
      <span className="sr-only">Centre frequency</span>
      <input
        type="text"
        inputMode="decimal"
        aria-label="Centre frequency"
        aria-invalid={invalid}
        spellCheck={false}
        placeholder="MHz"
        className={`w-36 rounded border bg-neutral-900 px-2 py-0.5 text-right font-mono text-base tabular-nums ${
          invalid ? "border-red-500" : "border-neutral-600"
        }`}
        value={shown}
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => {
          setDraft(e.target.value);
          setInvalid(false);
        }}
        onBlur={cancel}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            cancel();
            e.currentTarget.blur();
          } else if (e.key === "Enter") {
            const hz = parseFrequency(draft ?? shown);
            if (hz === null) {
              setInvalid(true);
              return;
            }
            onCommit(hz);
            cancel();
            e.currentTarget.blur();
          }
        }}
      />
      <span className="text-neutral-500">MHz</span>
    </label>
  );
}

export function TuningControls({ tuning, onTune }: Props) {
  const { display, busy, error } = tuning;
  const rates =
    display && !SAMPLE_RATES.includes(display.sampleRate)
      ? [...SAMPLE_RATES, display.sampleRate].sort((a, b) => a - b)
      : SAMPLE_RATES;
  const gain = display?.gain ?? null;
  const gainValue = gain === null ? "" : gain.mode === "auto" ? "auto" : String(gain.db);
  const gains = display?.gainsDb ?? [];
  const gainOptions =
    gain?.mode === "manual" && gain.db !== null && !gains.includes(gain.db)
      ? [...gains, gain.db].sort((a, b) => a - b)
      : gains;

  return (
    <div
      className="flex flex-wrap items-center gap-3 text-sm text-neutral-300"
      role="group"
      aria-label="Tuning"
    >
      <FrequencyInput
        centerHz={display?.centerHz ?? null}
        onCommit={(hz) => onTune({ centerHz: hz })}
      />
      <label className="flex items-center gap-1">
        Rate
        <select
          className={selectClass}
          disabled={!display || busy}
          value={display ? String(display.sampleRate) : ""}
          onChange={(e) => onTune({ sampleRate: Number(e.target.value) })}
        >
          {!display && <option value="">–</option>}
          {rates.map((r) => (
            <option key={r} value={r}>
              {formatRate(r)}
            </option>
          ))}
        </select>
      </label>
      <label className="flex items-center gap-1">
        Gain
        <select
          className={selectClass}
          disabled={!display || gain === null || busy}
          value={gainValue}
          onChange={(e) =>
            onTune({ gain: e.target.value === "auto" ? "auto" : Number(e.target.value) })
          }
        >
          {gain === null && <option value="">n/a</option>}
          {gain !== null && <option value="auto">Auto</option>}
          {gain !== null &&
            gainOptions.map((g) => (
              <option key={g} value={String(g)}>
                {g.toFixed(1)} dB
              </option>
            ))}
        </select>
      </label>
      {error && (
        <span className="text-red-400" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}
