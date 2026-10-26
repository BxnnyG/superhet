// Operating controls of one VFO, shared by the tuning strip (desktop) and the Receiver tab of the
// inspector (phones). Each control edits the VFO it is given; none owns state besides Auto squelch.

import { useSyncExternalStore } from "react";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import {
  Button,
  IconButton,
  SegmentedControl,
  Select,
  Slider,
  SpeakerIcon,
  SpeakerMutedIcon,
} from "../ui";
import { VFO_MODES, type Vfo, type VfoMode, type VfoPatch } from "./api";
import type { AutoSquelch } from "./autoSquelch";
import { LiveMarker } from "./LiveMarker";
import {
  BANDWIDTH_PRESETS,
  MODE_LABEL,
  SQUELCH_MIN_DBFS,
  SQUELCH_OFF_POS,
  formatBandwidth,
  formatSquelch,
  sliderToSquelch,
  squelchToSlider,
} from "./model";

const DEFAULT_BW = "default";

type Update = (id: number, patch: VfoPatch) => void;

/** Mode chips WFM / NFM / AM / USB / LSB / CW; a mode change resets the bandwidth to its default. */
export function ModeSelect({ vfo, onUpdate }: { vfo: Vfo; onUpdate: Update }) {
  return (
    <SegmentedControl
      aria-label="Mode"
      size="sm"
      value={vfo.mode}
      options={VFO_MODES.map((m) => ({ value: m, label: MODE_LABEL[m] }))}
      onChange={(m: VfoMode) => onUpdate(vfo.id, { mode: m, bandwidthHz: null })}
    />
  );
}

export function BandwidthSelect({
  vfo,
  onUpdate,
  className = "w-28 shrink-0",
}: {
  vfo: Vfo;
  onUpdate: Update;
  className?: string;
}) {
  const presets = BANDWIDTH_PRESETS[vfo.mode];
  const bandwidths = presets.includes(vfo.bandwidthHz)
    ? presets
    : [...presets, vfo.bandwidthHz].sort((a, b) => a - b);
  return (
    <Select
      aria-label="Bandwidth"
      size="sm"
      className={className}
      value={String(vfo.bandwidthHz)}
      onChange={(v) => onUpdate(vfo.id, { bandwidthHz: v === DEFAULT_BW ? null : Number(v) })}
      options={[
        ...bandwidths.map((b) => ({ value: String(b), label: formatBandwidth(b) })),
        { value: DEFAULT_BW, label: "Mode default" },
      ]}
    />
  );
}

/**
 * Squelch slider with an Auto button: Auto sets the threshold just above the noise floor seen in
 * the last few seconds. It stays marked "auto" until the slider moves; clicking it again (or
 * moving the slider to off) clears the squelch.
 */
export function SquelchControl({
  vfo,
  audio,
  autoSquelch,
  onUpdate,
  className,
}: {
  vfo: Vfo;
  audio: AudioController;
  autoSquelch: AutoSquelch;
  onUpdate: Update;
  className?: string;
}) {
  useSyncExternalStore(autoSquelch.subscribe, autoSquelch.getSnapshot);
  const auto = autoSquelch.isAuto(vfo);
  const onAuto = () => autoSquelch.toggle(vfo, audio, onUpdate);
  return (
    <div className={className ?? "flex items-end gap-2"}>
      <Slider
        className="min-w-0 flex-1"
        label="Squelch"
        min={SQUELCH_OFF_POS}
        max={0}
        value={squelchToSlider(vfo.squelchDbfs)}
        valueLabel={
          auto ? `auto ${formatSquelch(vfo.squelchDbfs)}` : formatSquelch(vfo.squelchDbfs)
        }
        title={`off, ${SQUELCH_MIN_DBFS}..0 dBFS (channel power); the green tick is the live level`}
        onChange={(pos) => {
          autoSquelch.forget(vfo.id);
          onUpdate(vfo.id, { squelchDbfs: sliderToSquelch(pos) });
        }}
        overlay={<LiveMarker audio={audio} vfoId={vfo.id} />}
      />
      <Button
        size="sm"
        variant={auto ? "primary" : "secondary"}
        aria-pressed={auto}
        title={
          auto
            ? "Auto squelch is on. Click to switch the squelch off"
            : "Set the squelch just above the noise floor of the last few seconds"
        }
        onClick={onAuto}
      >
        Auto
      </Button>
    </div>
  );
}

/** Per-VFO volume slider and mute toggle. */
export function VolumeControl({
  vfo,
  audio,
  audioSnap,
  className = "w-full",
}: {
  vfo: Vfo;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  className?: string;
}) {
  const level = audioSnap.levels[vfo.id] ?? { volume: 0.8, muted: false };
  return (
    <div className="flex items-end gap-2">
      <Slider
        className={className}
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
  );
}

/** Speaker toggle of a VFO: whether it is part of the audio mix. */
export function ListenToggle({
  id,
  listening,
  onListen,
  className,
}: {
  id: number;
  listening: boolean;
  onListen: (id: number, on: boolean) => void;
  className?: string;
}) {
  return (
    <IconButton
      label={`Listen to VFO ${id}`}
      tip={
        listening ? `Listening to VFO ${id}. Click to leave it out` : `Not listening to VFO ${id}`
      }
      size="sm"
      className={className}
      pressed={listening}
      onClick={() => onListen(id, !listening)}
    >
      {listening ? <SpeakerIcon size={14} /> : <SpeakerMutedIcon size={14} />}
    </IconButton>
  );
}
