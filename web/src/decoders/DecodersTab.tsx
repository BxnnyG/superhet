import type { AudioController, AudioSnapshot } from "../audio/controller";
import type { AnnotationTarget } from "../spectrum/annotations";
import type { Vfo, VfoPatch } from "../vfo/api";
import { EventLog } from "./EventLog";
import { LiveCards } from "./LiveCards";
import type { DecoderEventStore } from "./store";

interface Props {
  vfos: readonly Vfo[];
  selectedId: number | null;
  audio: AudioController;
  audioSnap: AudioSnapshot;
  events: DecoderEventStore;
  device: string;
  onUpdate: (id: number, patch: VfoPatch) => void;
  onTune: (target: AnnotationTarget) => void;
}

/**
 * Decoders tab: live cards of the decoding receivers on top (RDS, CTCSS, DTMF), then the
 * persistent event log. The cards scroll within a capped area so the log keeps its room.
 */
export function DecodersTab({
  vfos,
  selectedId,
  audio,
  audioSnap,
  events,
  device,
  onUpdate,
  onTune,
}: Props) {
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="max-h-[48%] shrink-0 overflow-y-auto border-b border-border">
        <LiveCards
          vfos={vfos}
          selectedId={selectedId}
          audio={audio}
          audioSnap={audioSnap}
          onUpdate={onUpdate}
        />
      </div>
      <EventLog store={events} device={device} onTune={onTune} />
    </div>
  );
}
