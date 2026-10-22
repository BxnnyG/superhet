import type { AudioController, AudioSnapshot } from "../audio/controller";
import { formatFrequency } from "../spectrum/freq";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  Badge,
  Button,
  EmptyState,
  Field,
  IconButton,
  Kbd,
  PauseIcon,
  PlusIcon,
  RowButton,
  SegmentedControl,
  Select,
  Slider,
  SpeakerIcon,
  SpeakerMutedIcon,
  TrashIcon,
  cn,
} from "../ui";
import { VFO_MODES, type Vfo, type VfoMode, type VfoPatch } from "./api";
import type { VfoSnapshot } from "./controller";
import { LiveMarker } from "./LiveMarker";
import {
  BANDWIDTH_PRESETS,
  MODE_LABEL,
  STEPS_HZ,
  VFO_PAUSED_HINT,
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

const DEFAULT_BW = "default";
const PAUSED_FREE_HINT = "VFO outside the usable window — paused until the device is retuned";

interface Props {
  vfos: VfoSnapshot;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  step: StepChoice;
  onStep: (step: StepChoice) => void;
  /** Maximum number of VFOs per device (daemon limit). */
  maxVfos?: number;
  /** The tuning lock is held: a VFO outside the window stays paused instead of retuning. */
  locked?: boolean;
  onCreate: () => void;
  onSelect: (id: number | null) => void;
  onUpdate: (id: number, patch: VfoPatch) => void;
  onRemove: (id: number) => void;
}

/** Radio section content: VFO list, tuning step and the controls of the selected VFO. */
export function VfoPanel({
  vfos,
  audio,
  audioSnap,
  step,
  onStep,
  maxVfos = 8,
  locked = false,
  onCreate,
  onSelect,
  onUpdate,
  onRemove,
}: Props) {
  const selected = vfos.vfos.find((v) => v.id === vfos.selectedId) ?? null;
  const full = vfos.vfos.length >= maxVfos;
  const pausedHint = locked ? VFO_PAUSED_HINT : PAUSED_FREE_HINT;
  const anyPaused = vfos.vfos.some((v) => v.status === "paused");
  return (
    <div className="flex flex-col gap-3" role="group" aria-label="VFOs">
      <div className="flex items-end gap-2">
        <Select
          className="flex-1"
          label="Tuning step"
          value={String(step)}
          onChange={(v) => onStep(v === "auto" ? "auto" : Number(v))}
          options={[
            { value: "auto", label: "Auto (by mode)" },
            ...STEPS_HZ.map((s) => ({ value: String(s), label: formatStep(s) })),
          ]}
        />
        <Button
          variant="primary"
          icon={<PlusIcon />}
          disabled={full}
          title="New VFO at the display centre (Ctrl/Alt-click on the spectrum creates one at the pointer)"
          onClick={onCreate}
        >
          VFO
        </Button>
      </div>
      {vfos.vfos.length === 0 ? (
        <EmptyState title="No VFOs yet">
          {vfos.loaded ? (
            <>
              <Kbd>Ctrl</Kbd>+click the spectrum or press <Kbd>+ VFO</Kbd>.
            </>
          ) : (
            "Loading…"
          )}
        </EmptyState>
      ) : (
        <ul className="flex flex-col gap-1" aria-label="VFO list">
          {vfos.vfos.map((v) => {
            const isSel = v.id === vfos.selectedId;
            return (
              <li key={v.id} className="flex items-center gap-1">
                <RowButton
                  selected={isSel}
                  onClick={() => onSelect(isSel ? null : v.id)}
                  className={cn("flex-1", v.status === "paused" && "italic opacity-70")}
                >
                  <span
                    aria-hidden="true"
                    className="size-2.5 shrink-0 rounded-full"
                    style={{ background: vfoColor(v.id) }}
                  />
                  <span className="w-4 text-xs text-fg-subtle num">{v.id}</span>
                  <Badge tone={isSel ? "accent" : "neutral"}>{MODE_LABEL[v.mode]}</Badge>
                  <span className="min-w-0 flex-1 truncate text-right font-mono text-sm text-fg num">
                    {formatFrequency(v.freqHz).slice(0, -4)}
                  </span>
                  {v.status === "paused" && (
                    <Badge tone="warn" title={pausedHint}>
                      <PauseIcon size={10} />
                      paused
                    </Badge>
                  )}
                </RowButton>
                <IconButton label={`Delete VFO ${v.id}`} size="sm" onClick={() => onRemove(v.id)}>
                  <TrashIcon size={14} />
                </IconButton>
              </li>
            );
          })}
        </ul>
      )}
      {anyPaused && (
        <p role="status" className="flex items-start gap-1.5 text-xs text-warn">
          <PauseIcon size={14} className="mt-px shrink-0" />
          <span>{pausedHint}</span>
        </p>
      )}
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
        vfos.vfos.length > 0 && (
          <p className="text-xs text-fg-subtle">
            No VFO selected: a plain click on the spectrum tunes the device centre.
          </p>
        )
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
      className="flex flex-col gap-3 rounded-lg border border-border bg-surface-2/50 p-3"
      role="group"
      aria-label={`VFO ${vfo.id}`}
    >
      <div className="flex items-center justify-between gap-2">
        <IconButton label="Down one step" variant="secondary" onClick={() => nudge(-1)}>
          <ArrowLeftIcon />
        </IconButton>
        <div className="flex flex-col items-center">
          <span className="font-mono text-lg text-fg num">{formatFrequency(vfo.freqHz)}</span>
          <span className="text-[11px] text-fg-subtle">MHz · step {formatStep(stepHz)}</span>
        </div>
        <IconButton label="Up one step" variant="secondary" onClick={() => nudge(1)}>
          <ArrowRightIcon />
        </IconButton>
      </div>
      <Field label="Mode">
        <SegmentedControl
          aria-label="Mode"
          value={vfo.mode}
          options={VFO_MODES.map((m) => ({ value: m, label: MODE_LABEL[m] }))}
          // A mode change resets the bandwidth to the new mode's default.
          onChange={(m: VfoMode) => onUpdate(vfo.id, { mode: m, bandwidthHz: null })}
          className="w-full"
        />
      </Field>
      <Select
        label="Bandwidth"
        value={String(vfo.bandwidthHz)}
        onChange={(v) => onUpdate(vfo.id, { bandwidthHz: v === DEFAULT_BW ? null : Number(v) })}
        options={[
          ...bandwidths.map((b) => ({ value: String(b), label: formatBandwidth(b) })),
          { value: DEFAULT_BW, label: "Mode default" },
        ]}
      />
      <Slider
        label="Squelch"
        min={SQUELCH_OFF_POS}
        max={0}
        value={squelchToSlider(vfo.squelchDbfs)}
        valueLabel={formatSquelch(vfo.squelchDbfs)}
        title={`off, ${SQUELCH_MIN_DBFS}..0 dBFS (channel power); the green tick is the live level`}
        onChange={(pos) => onUpdate(vfo.id, { squelchDbfs: sliderToSquelch(pos) })}
        overlay={<LiveMarker audio={audio} vfoId={vfo.id} />}
      />
      <div className="flex items-end gap-2">
        <Slider
          className="flex-1"
          label="Volume"
          min={0}
          max={100}
          value={Math.round(level.volume * 100)}
          valueLabel={level.muted ? "muted" : `${Math.round(level.volume * 100)} %`}
          onChange={(v) => audio.setVolume(vfo.id, v / 100)}
        />
        <IconButton
          label={level.muted ? "Unmute" : "Mute"}
          variant="secondary"
          pressed={level.muted}
          onClick={() => audio.setMuted(vfo.id, !level.muted)}
        >
          {level.muted ? <SpeakerMutedIcon /> : <SpeakerIcon />}
        </IconButton>
      </div>
      <SMeter audio={audio} vfoId={vfo.id} />
      <div className="flex gap-2">
        <Button variant="secondary" size="sm" className="flex-1" onClick={onDeselect} title="Esc">
          Deselect
        </Button>
        <Button
          variant="danger"
          size="sm"
          className="flex-1"
          icon={<TrashIcon size={14} />}
          onClick={() => onRemove(vfo.id)}
        >
          Delete
        </Button>
      </div>
    </div>
  );
}
