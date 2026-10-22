import type { ReactNode } from "react";
import { AudioButton } from "../audio/AudioControls";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import { formatFrequency } from "../spectrum/freq";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  Badge,
  Button,
  Chip,
  FrequencyReadout,
  IconButton,
  PauseIcon,
  PlusIcon,
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
  SQUELCH_MIN_DBFS,
  SQUELCH_OFF_POS,
  STEPS_HZ,
  VFO_PAUSED_FREE_HINT,
  VFO_PAUSED_HINT,
  formatBandwidth,
  formatSquelch,
  formatStep,
  sliderToSquelch,
  snapFrequency,
  squelchToSlider,
  stepFor,
  vfoColor,
  type StepChoice,
} from "./model";
import { SMeter } from "./SMeter";

const DEFAULT_BW = "default";

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
  onSelect: (id: number) => void;
  onUpdate: (id: number, patch: VfoPatch) => void;
  onRemove: (id: number) => void;
}

/** Persistent operating controls of the active VFO, docked below the spectrum. */
export function ReceiverDock({
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
  const active = vfos.vfos.find((v) => v.id === vfos.selectedId) ?? null;
  const full = vfos.vfos.length >= maxVfos;
  const pausedHint = locked ? VFO_PAUSED_HINT : VFO_PAUSED_FREE_HINT;

  if (vfos.vfos.length === 0) {
    return (
      <section
        aria-label="Receiver"
        className="flex shrink-0 flex-wrap items-center justify-between gap-3 rounded-xl border border-border bg-surface-1 p-3"
      >
        <p className="text-sm text-fg-muted">
          {vfos.loaded
            ? "No VFO yet. Add one to start listening, or Ctrl+click the spectrum."
            : "Loading…"}
        </p>
        <Button variant="primary" icon={<PlusIcon />} disabled={!vfos.loaded} onClick={onCreate}>
          Add VFO
        </Button>
      </section>
    );
  }

  return (
    <section
      aria-label="Receiver"
      className="@container shrink-0 rounded-xl border border-border bg-surface-1"
    >
      <div className="flex flex-col @[88rem]:flex-row @[88rem]:items-stretch">
        <Band className="@[88rem]:flex-1">
          <Group label="VFO selection">
            <ul className="flex flex-wrap items-center gap-1.5">
              {vfos.vfos.map((v) => (
                <li key={v.id}>
                  <VfoChip vfo={v} selected={v.id === vfos.selectedId} onSelect={onSelect} />
                </li>
              ))}
            </ul>
            <IconButton
              label="Add VFO"
              tip="New VFO at the display centre (Ctrl/Alt-click on the spectrum creates one at the pointer)"
              variant="secondary"
              disabled={full}
              onClick={onCreate}
            >
              <PlusIcon />
            </IconButton>
          </Group>
          {active && (
            <Group divider>
              <Tuner vfo={active} step={step} onStep={onStep} onUpdate={onUpdate} />
              {active.status === "paused" && (
                <Badge tone="warn" title={pausedHint}>
                  <PauseIcon size={10} />
                  paused
                </Badge>
              )}
            </Group>
          )}
        </Band>
        {active && (
          <Band border className="@[88rem]:flex-1 @[88rem]:border-t-0 @[88rem]:border-l">
            <Group label="Demodulator">
              <SegmentedControl
                aria-label="Mode"
                size="sm"
                value={active.mode}
                options={VFO_MODES.map((m) => ({ value: m, label: MODE_LABEL[m] }))}
                // A mode change resets the bandwidth to the new mode's default.
                onChange={(m: VfoMode) => onUpdate(active.id, { mode: m, bandwidthHz: null })}
              />
              <Bandwidth vfo={active} onUpdate={onUpdate} />
            </Group>
            <Group divider label="Squelch and signal" className="basis-full @lg:basis-auto">
              <div className="flex w-full flex-col gap-1 @lg:w-44">
                <Slider
                  label="Squelch"
                  min={SQUELCH_OFF_POS}
                  max={0}
                  value={squelchToSlider(active.squelchDbfs)}
                  valueLabel={formatSquelch(active.squelchDbfs)}
                  title={`off, ${SQUELCH_MIN_DBFS}..0 dBFS (channel power); the green tick is the live level`}
                  onChange={(pos) => onUpdate(active.id, { squelchDbfs: sliderToSquelch(pos) })}
                  overlay={<LiveMarker audio={audio} vfoId={active.id} />}
                />
                <SMeter audio={audio} vfoId={active.id} compact />
              </div>
            </Group>
            <Group divider label="Output">
              <Volume vfo={active} audio={audio} audioSnap={audioSnap} />
              <AudioButton audio={audio} snapshot={audioSnap} />
              <IconButton
                label={`Delete VFO ${active.id}`}
                variant="ghost"
                onClick={() => onRemove(active.id)}
              >
                <TrashIcon size={16} />
              </IconButton>
            </Group>
          </Band>
        )}
      </div>
    </section>
  );
}

/** One row of groups; wraps cleanly when the dock is narrow. */
function Band({
  border,
  className,
  children,
}: {
  border?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-x-4 gap-y-2 px-3 py-2.5",
        border && "border-t border-border",
        className,
      )}
    >
      {children}
    </div>
  );
}

/** Control group; separated from its predecessor by a subtle divider once the band is wide. */
function Group({
  label,
  divider,
  className,
  children,
}: {
  label?: string;
  divider?: boolean;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className={cn(
        "flex min-w-0 flex-wrap items-center gap-2",
        divider && "@4xl:border-l @4xl:border-border @4xl:pl-4",
        className,
      )}
    >
      {children}
    </div>
  );
}

function VfoChip({
  vfo,
  selected,
  onSelect,
}: {
  vfo: Vfo;
  selected: boolean;
  onSelect: (id: number) => void;
}) {
  const paused = vfo.status === "paused";
  return (
    <Chip
      selected={selected}
      muted={paused}
      aria-label={`VFO ${vfo.id} ${MODE_LABEL[vfo.mode]} ${formatFrequency(vfo.freqHz)} MHz${paused ? " (paused)" : ""}`}
      title={paused ? "Paused: outside the usable window" : undefined}
      onClick={() => onSelect(vfo.id)}
    >
      <span
        aria-hidden="true"
        className="size-2 shrink-0 rounded-full"
        style={{ background: vfoColor(vfo.id) }}
      />
      <span className="text-fg-subtle num">{vfo.id}</span>
      <span className="font-mono num">{formatFrequency(vfo.freqHz).slice(0, -4)}</span>
      {paused && <PauseIcon size={10} />}
    </Chip>
  );
}

function Tuner({
  vfo,
  step,
  onStep,
  onUpdate,
}: {
  vfo: Vfo;
  step: StepChoice;
  onStep: (step: StepChoice) => void;
  onUpdate: (id: number, patch: VfoPatch) => void;
}) {
  const stepHz = stepFor(step, vfo.mode);
  const nudge = (dir: 1 | -1) =>
    onUpdate(vfo.id, { freqHz: snapFrequency(vfo.freqHz + dir * stepHz, stepHz) });
  return (
    <>
      <span className="flex shrink-0 items-center gap-1.5">
        <IconButton label="Down one step" variant="secondary" onClick={() => nudge(-1)}>
          <ArrowLeftIcon />
        </IconButton>
        <FrequencyReadout
          key={vfo.id}
          label="VFO frequency"
          valueHz={vfo.freqHz}
          onChange={(hz) => onUpdate(vfo.id, { freqHz: hz })}
        />
        <IconButton label="Up one step" variant="secondary" onClick={() => nudge(1)}>
          <ArrowRightIcon />
        </IconButton>
      </span>
      <Select
        aria-label="Tuning step"
        size="sm"
        className="w-32"
        value={String(step)}
        onChange={(v) => onStep(v === "auto" ? "auto" : Number(v))}
        options={[
          { value: "auto", label: `Auto (${formatStep(stepFor("auto", vfo.mode))})` },
          ...STEPS_HZ.map((s) => ({ value: String(s), label: formatStep(s) })),
        ]}
      />
    </>
  );
}

function Bandwidth({
  vfo,
  onUpdate,
}: {
  vfo: Vfo;
  onUpdate: (id: number, patch: VfoPatch) => void;
}) {
  const presets = BANDWIDTH_PRESETS[vfo.mode];
  const bandwidths = presets.includes(vfo.bandwidthHz)
    ? presets
    : [...presets, vfo.bandwidthHz].sort((a, b) => a - b);
  return (
    <Select
      aria-label="Bandwidth"
      size="sm"
      className="w-28"
      value={String(vfo.bandwidthHz)}
      onChange={(v) => onUpdate(vfo.id, { bandwidthHz: v === DEFAULT_BW ? null : Number(v) })}
      options={[
        ...bandwidths.map((b) => ({ value: String(b), label: formatBandwidth(b) })),
        { value: DEFAULT_BW, label: "Mode default" },
      ]}
    />
  );
}

function Volume({
  vfo,
  audio,
  audioSnap,
}: {
  vfo: Vfo;
  audio: AudioController;
  audioSnap: AudioSnapshot;
}) {
  const level = audioSnap.levels[vfo.id] ?? { volume: 0.8, muted: false };
  return (
    <>
      <Slider
        className="w-32"
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
    </>
  );
}
