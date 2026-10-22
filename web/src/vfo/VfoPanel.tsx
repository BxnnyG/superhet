import { formatFrequency } from "../spectrum/freq";
import {
  Badge,
  Button,
  EmptyState,
  IconButton,
  PauseIcon,
  PlusIcon,
  RowButton,
  SpeakerIcon,
  SpeakerMutedIcon,
  TrashIcon,
  cn,
} from "../ui";
import type { VfoSnapshot } from "./controller";
import { MODE_LABEL, VFO_PAUSED_FREE_HINT, VFO_PAUSED_HINT, vfoColor } from "./model";

interface Props {
  vfos: VfoSnapshot;
  /** Maximum number of VFOs per device (daemon limit). */
  maxVfos?: number;
  /** The tuning lock is held: a VFO outside the window stays paused instead of retuning. */
  locked?: boolean;
  /** Whether a VFO is part of the audio mix. */
  isListening: (id: number) => boolean;
  onListen: (id: number, on: boolean) => void;
  onCreate: () => void;
  onSelect: (id: number) => void;
  onRemove: (id: number) => void;
}

/** Radio section content: compact VFO list (the active VFO is edited in the receiver dock). */
export function VfoPanel({
  vfos,
  maxVfos = 8,
  locked = false,
  isListening,
  onListen,
  onCreate,
  onSelect,
  onRemove,
}: Props) {
  const full = vfos.vfos.length >= maxVfos;
  const pausedHint = locked ? VFO_PAUSED_HINT : VFO_PAUSED_FREE_HINT;
  const anyPaused = vfos.vfos.some((v) => v.status === "paused");
  return (
    <div className="flex flex-col gap-3" role="group" aria-label="VFOs">
      <Button
        variant="primary"
        icon={<PlusIcon />}
        disabled={full}
        title="New VFO at the display centre (Ctrl/Alt-click on the spectrum creates one at the pointer)"
        onClick={onCreate}
      >
        VFO
      </Button>
      {vfos.vfos.length === 0 ? (
        <EmptyState title="No VFOs yet">
          {vfos.loaded ? "Click the spectrum or press + to add a VFO" : "Loading…"}
        </EmptyState>
      ) : (
        <ul className="flex flex-col gap-1" aria-label="VFO list">
          {vfos.vfos.map((v) => {
            const isSel = v.id === vfos.selectedId;
            const listening = isListening(v.id);
            return (
              <li key={v.id} className="flex items-center gap-1">
                <RowButton
                  selected={isSel}
                  onClick={() => onSelect(v.id)}
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
                <IconButton
                  label={`Listen to VFO ${v.id}`}
                  tip={listening ? "Listening. Click to leave this VFO out" : "Not listening"}
                  size="sm"
                  pressed={listening}
                  onClick={() => onListen(v.id, !listening)}
                >
                  {listening ? <SpeakerIcon size={14} /> : <SpeakerMutedIcon size={14} />}
                </IconButton>
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
    </div>
  );
}
