import { useSyncExternalStore } from "react";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import { usePermissions } from "../auth/context";
import { DecoderPicker } from "../plugins/DecoderPicker";
import type { PluginStore } from "../plugins/store";
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
  plugins: PluginStore;
  device: string;
  onUpdate: (id: number, patch: VfoPatch) => void;
  onTune: (target: AnnotationTarget) => void;
}

/**
 * Decoders tab: the plugin picker for the selected VFO (operators and admins) and the live cards
 * of the decoding receivers (RDS, CTCSS, DTMF) on top, then the persistent event log. The top
 * part scrolls within a capped area so the log keeps its room.
 */
export function DecodersTab({
  vfos,
  selectedId,
  audio,
  audioSnap,
  events,
  plugins,
  device,
  onUpdate,
  onTune,
}: Props) {
  const perms = usePermissions();
  const pluginSnap = useSyncExternalStore(plugins.subscribe, plugins.getSnapshot);
  const selected = vfos.find((v) => v.id === selectedId) ?? null;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="max-h-[48%] shrink-0 overflow-y-auto border-b border-border">
        {perms.canUsePlugins && (
          <DecoderPicker
            store={plugins}
            device={device}
            vfo={selected}
            isAdmin={perms.isAdmin}
            onUpdateVfo={onUpdate}
          />
        )}
        <LiveCards
          vfos={vfos}
          selectedId={selectedId}
          audio={audio}
          audioSnap={audioSnap}
          onUpdate={onUpdate}
        />
      </div>
      <EventLog store={events} device={device} plugins={pluginSnap.plugins} onTune={onTune} />
    </div>
  );
}
