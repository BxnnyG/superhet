import type { ReactNode } from "react";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import { Badge, Button, EmptyState, PauseIcon, TrashIcon, cn } from "../ui";
import type { Vfo, VfoPatch } from "./api";
import type { AutoSquelch } from "./autoSquelch";
import {
  BandwidthSelect,
  CwPitchControl,
  ModeSelect,
  SquelchControl,
  VolumeControl,
} from "./controls";
import { MODE_LABEL, VFO_PAUSED_FREE_HINT, VFO_PAUSED_HINT, vfoColor } from "./model";
import { RdsPanel } from "./RdsPanel";
import { SMeter } from "./SMeter";
import { StereoControl } from "./StereoControl";
import { TonesPanel } from "./TonesPanel";

type Update = (id: number, patch: VfoPatch) => void;

interface Props {
  /** The selected VFO; null shows the empty state. */
  vfo: Vfo | null;
  loaded: boolean;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  autoSquelch: AutoSquelch;
  locked: boolean;
  /** Also show mode, bandwidth and squelch (phones: the tuning strip only has the frequency). */
  showDemodulator: boolean;
  onUpdate: Update;
  onRemove: (id: number) => void;
}

/** Receiver tab of the inspector: everything about the selected VFO that the strip does not show. */
export function ReceiverPanel({
  vfo,
  loaded,
  audio,
  audioSnap,
  autoSquelch,
  locked,
  showDemodulator,
  onUpdate,
  onRemove,
}: Props) {
  if (vfo === null) {
    return (
      <div className="flex flex-col gap-3 p-4">
        <EmptyState title="No receiver yet">
          {loaded ? "Click the spectrum or press N to add a VFO" : "Loading…"}
        </EmptyState>
      </div>
    );
  }
  return (
    <div className="flex flex-col" role="group" aria-label="Selected receiver">
      <Block>
        <div className="flex items-center gap-2">
          <span
            aria-hidden="true"
            className="size-2.5 shrink-0 rounded-full"
            style={{ background: vfoColor(vfo.id) }}
          />
          <span className="text-sm font-medium">VFO {vfo.id}</span>
          <Badge tone="accent">{MODE_LABEL[vfo.mode]}</Badge>
          {vfo.status === "paused" && (
            <Badge tone="warn" title={locked ? VFO_PAUSED_HINT : VFO_PAUSED_FREE_HINT}>
              <PauseIcon size={10} />
              paused
            </Badge>
          )}
        </div>
      </Block>
      {showDemodulator && (
        <Block title="Demodulator">
          <ModeSelect vfo={vfo} onUpdate={onUpdate} />
          <BandwidthSelect vfo={vfo} onUpdate={onUpdate} className="w-full" />
          <div className="flex flex-col gap-1">
            <SquelchControl vfo={vfo} audio={audio} autoSquelch={autoSquelch} onUpdate={onUpdate} />
            <SMeter audio={audio} vfoId={vfo.id} compact />
          </div>
        </Block>
      )}
      {vfo.mode === "wfm" && (
        <Block title="Stereo and de-emphasis">
          <StereoControl vfo={vfo} audio={audio} onUpdate={onUpdate} />
          <p className="text-xs text-fg-muted">
            De-emphasis <span className="font-mono text-fg">{vfo.deemphasis ?? "default"}</span>
          </p>
        </Block>
      )}
      {vfo.mode === "cw" && (
        <Block title="CW">
          <CwPitchControl vfo={vfo} onUpdate={onUpdate} />
        </Block>
      )}
      <Block title="Output">
        <VolumeControl vfo={vfo} audio={audio} audioSnap={audioSnap} />
      </Block>
      <Decoders vfo={vfo} audio={audio} audioSnap={audioSnap} onUpdate={onUpdate} />
      <Block>
        <Button
          variant="danger"
          icon={<TrashIcon size={14} />}
          className="self-start"
          onClick={() => onRemove(vfo.id)}
        >
          Remove VFO
        </Button>
      </Block>
    </div>
  );
}

/** RDS (WFM) or CTCSS / DTMF (NFM, AM) of the selected VFO; used by the Receiver and Decoders tabs. */
export function Decoders({
  vfo,
  audio,
  audioSnap,
  onUpdate,
}: {
  vfo: Vfo;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  onUpdate: Update;
}) {
  if (vfo.mode === "wfm") {
    return (
      <Block flush>
        <RdsPanel rds={audioSnap.rds[vfo.id]} deemphasis={vfo.deemphasis} />
      </Block>
    );
  }
  if (vfo.mode === "nfm" || vfo.mode === "am") {
    return (
      <Block flush>
        <TonesPanel vfo={vfo} tones={audioSnap.tones[vfo.id]} audio={audio} onUpdate={onUpdate} />
      </Block>
    );
  }
  return null;
}

function Block({
  title,
  flush,
  children,
}: {
  title?: string;
  /** No own padding (the content brings its own). */
  flush?: boolean;
  children: ReactNode;
}) {
  return (
    <section className={cn("flex flex-col gap-3 border-b border-border", !flush && "px-4 py-3")}>
      {title && (
        <h3 className="text-xs font-semibold tracking-wider text-fg-muted uppercase">{title}</h3>
      )}
      {children}
    </section>
  );
}
