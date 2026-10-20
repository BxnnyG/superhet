import type { AudioController, AudioSnapshot } from "../audio/controller";
import { FrequencyInput } from "../spectrum/TuningControls";
import { formatFrequency } from "../spectrum/freq";
import { VFO_MODES, type Vfo, type VfoMode, type VfoPatch } from "./api";
import type { VfoSnapshot } from "./controller";
import {
  BANDWIDTH_PRESETS,
  MODE_LABEL,
  STEPS_HZ,
  SQUELCH_MIN_DBFS,
  SQUELCH_OFF_POS,
  formatBandwidth,
  formatSquelch,
  formatStep,
  snapFrequency,
  sliderToSquelch,
  squelchToSlider,
  stepFor,
  vfoColor,
  type StepChoice,
} from "./model";
import { SMeter } from "./SMeter";

const selectClass = "rounded border border-neutral-600 bg-neutral-900 px-1 py-0.5";
const btnClass = "rounded border border-neutral-600 px-2 py-0.5 disabled:opacity-40";
const DEFAULT_BW = "default";

interface Props {
  vfos: VfoSnapshot;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  step: StepChoice;
  onStep: (step: StepChoice) => void;
  /** Maximum number of VFOs per device (daemon limit). */
  maxVfos?: number;
  onCreate: () => void;
  onSelect: (id: number | null) => void;
  onUpdate: (id: number, patch: VfoPatch) => void;
  onRemove: (id: number) => void;
}

export function VfoPanel({
  vfos,
  audio,
  audioSnap,
  step,
  onStep,
  maxVfos = 8,
  onCreate,
  onSelect,
  onUpdate,
  onRemove,
}: Props) {
  const selected = vfos.vfos.find((v) => v.id === vfos.selectedId) ?? null;
  return (
    <div className="flex flex-col gap-2 text-sm text-neutral-300" role="group" aria-label="VFOs">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className={btnClass}
          disabled={vfos.vfos.length >= maxVfos}
          title="New VFO at the display centre (Alt/Ctrl-click on the spectrum creates one at the pointer)"
          onClick={onCreate}
        >
          + VFO
        </button>
        {vfos.vfos.map((v) => (
          <button
            key={v.id}
            type="button"
            aria-pressed={v.id === vfos.selectedId}
            className={`rounded border px-2 py-0.5 font-mono text-xs tabular-nums ${
              v.id === vfos.selectedId ? "bg-neutral-800" : ""
            } ${v.status === "paused" ? "opacity-50" : ""}`}
            style={{ borderColor: vfoColor(v.id) }}
            onClick={() => onSelect(v.id === vfos.selectedId ? null : v.id)}
          >
            {v.id} {MODE_LABEL[v.mode]} {formatFrequency(v.freqHz).slice(0, -4)}
          </button>
        ))}
        <label className="ml-auto flex items-center gap-1">
          Step
          <select
            className={selectClass}
            value={String(step)}
            onChange={(e) => onStep(e.target.value === "auto" ? "auto" : Number(e.target.value))}
          >
            <option value="auto">Auto (by mode)</option>
            {STEPS_HZ.map((s) => (
              <option key={s} value={s}>
                {formatStep(s)}
              </option>
            ))}
          </select>
        </label>
        {vfos.error && (
          <span className="text-red-400" role="alert">
            {vfos.error}
          </span>
        )}
      </div>
      {selected ? (
        <SelectedVfo
          vfo={selected}
          audio={audio}
          audioSnap={audioSnap}
          step={step}
          onUpdate={onUpdate}
          onRemove={onRemove}
          onDeselect={() => onSelect(null)}
        />
      ) : (
        <p className="text-xs text-neutral-500">
          {vfos.loaded && vfos.vfos.length === 0
            ? "No VFOs. Alt/Ctrl-click the spectrum or press + VFO to create one."
            : "No VFO selected: a plain click on the spectrum tunes the device centre."}
        </p>
      )}
    </div>
  );
}

function SelectedVfo({
  vfo,
  audio,
  audioSnap,
  step,
  onUpdate,
  onRemove,
  onDeselect,
}: {
  vfo: Vfo;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  step: StepChoice;
  onUpdate: (id: number, patch: VfoPatch) => void;
  onRemove: (id: number) => void;
  onDeselect: () => void;
}) {
  const stepHz = stepFor(step, vfo.mode);
  const level = audioSnap.levels[vfo.id] ?? { volume: 0.8, muted: false };
  const presets = BANDWIDTH_PRESETS[vfo.mode];
  const bandwidths = presets.includes(vfo.bandwidthHz)
    ? presets
    : [...presets, vfo.bandwidthHz].sort((a, b) => a - b);
  const nudge = (dir: 1 | -1) =>
    onUpdate(vfo.id, { freqHz: snapFrequency(vfo.freqHz + dir * stepHz, stepHz) });

  return (
    <div
      className="flex flex-wrap items-center gap-x-3 gap-y-2"
      role="group"
      aria-label={`VFO ${vfo.id}`}
    >
      <span className="flex items-center gap-1">
        <button
          type="button"
          className={btnClass}
          aria-label="Down one step"
          onClick={() => nudge(-1)}
        >
          ◀
        </button>
        <FrequencyInput
          label="VFO frequency"
          centerHz={vfo.freqHz}
          onCommit={(hz) => onUpdate(vfo.id, { freqHz: hz })}
        />
        <button
          type="button"
          className={btnClass}
          aria-label="Up one step"
          onClick={() => nudge(1)}
        >
          ▶
        </button>
      </span>
      <label className="flex items-center gap-1">
        Mode
        <select
          className={selectClass}
          value={vfo.mode}
          onChange={(e) =>
            // A mode change resets the bandwidth to the new mode's default.
            onUpdate(vfo.id, { mode: e.target.value as VfoMode, bandwidthHz: null })
          }
        >
          {VFO_MODES.map((m) => (
            <option key={m} value={m}>
              {MODE_LABEL[m]}
            </option>
          ))}
        </select>
      </label>
      <label className="flex items-center gap-1">
        Bandwidth
        <select
          className={selectClass}
          value={String(vfo.bandwidthHz)}
          onChange={(e) =>
            onUpdate(vfo.id, {
              bandwidthHz: e.target.value === DEFAULT_BW ? null : Number(e.target.value),
            })
          }
        >
          {bandwidths.map((b) => (
            <option key={b} value={b}>
              {formatBandwidth(b)}
            </option>
          ))}
          <option value={DEFAULT_BW}>Mode default</option>
        </select>
      </label>
      <label className="flex items-center gap-1">
        Squelch
        <input
          type="range"
          className="w-28"
          min={SQUELCH_OFF_POS}
          max={0}
          step={1}
          value={squelchToSlider(vfo.squelchDbfs)}
          aria-valuetext={formatSquelch(vfo.squelchDbfs)}
          title={`off, ${SQUELCH_MIN_DBFS}..0 dBFS (channel power)`}
          onChange={(e) =>
            onUpdate(vfo.id, { squelchDbfs: sliderToSquelch(Number(e.target.value)) })
          }
        />
        <output className="w-16 text-right font-mono text-xs tabular-nums">
          {formatSquelch(vfo.squelchDbfs)}
        </output>
      </label>
      <label className="flex items-center gap-1">
        Volume
        <input
          type="range"
          className="w-24"
          min={0}
          max={100}
          step={1}
          value={Math.round(level.volume * 100)}
          onChange={(e) => audio.setVolume(vfo.id, Number(e.target.value) / 100)}
        />
      </label>
      <label className="flex items-center gap-1">
        <input
          type="checkbox"
          checked={level.muted}
          onChange={(e) => audio.setMuted(vfo.id, e.target.checked)}
        />
        Mute
      </label>
      <SMeter audio={audio} vfoId={vfo.id} />
      {vfo.status === "paused" && (
        <span
          className="rounded bg-amber-900/60 px-1.5 py-0.5 text-xs text-amber-300"
          title="Outside the device passband: no audio until the device is retuned"
        >
          paused
        </span>
      )}
      <button type="button" className={btnClass} onClick={onDeselect} title="Esc">
        Deselect
      </button>
      <button type="button" className={`${btnClass} text-red-300`} onClick={() => onRemove(vfo.id)}>
        Delete
      </button>
    </div>
  );
}
