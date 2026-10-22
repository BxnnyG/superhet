import type { Vfo } from "../vfo/api";
import { MODE_LABEL, vfoColor } from "../vfo/model";
import { Badge, FrequencyReadout } from "../ui";

interface Props {
  /** Confirmed/optimistic device centre; null while unknown. */
  centerHz: number | null;
  /** The selected VFO, if any: the readout edits it instead of the centre. */
  selected: Vfo | null;
  onCenter: (hz: number) => void;
  onVfo: (id: number, hz: number) => void;
  /** Why the centre cannot be changed (tuning lock), or null. VFOs stay editable. */
  centerLockedReason?: string | null;
}

/** Big frequency readout routed to the selected VFO, else to the device centre. */
export function ReceiverFrequency({
  centerHz,
  selected,
  onCenter,
  onVfo,
  centerLockedReason = null,
}: Props) {
  const paused = selected?.status === "paused";
  return (
    <div className="flex items-center gap-3">
      <FrequencyReadout
        key={selected ? `vfo-${selected.id}` : "centre"}
        label={selected ? "VFO frequency" : "Centre frequency"}
        valueHz={selected ? selected.freqHz : centerHz}
        onChange={(hz) => (selected ? onVfo(selected.id, hz) : onCenter(hz))}
        locked={!selected && centerLockedReason !== null}
        lockedReason={centerLockedReason ?? undefined}
      />
      <Badge
        tone={selected ? (paused ? "warn" : "accent") : "neutral"}
        title={
          selected
            ? paused
              ? "The readout tunes the selected VFO, which is outside the usable window (paused)"
              : "The readout tunes the selected VFO"
            : "No VFO selected: the readout tunes the device centre"
        }
      >
        {selected ? (
          <>
            <span
              aria-hidden="true"
              className="size-1.5 rounded-full"
              style={{ background: vfoColor(selected.id) }}
            />
            VFO {selected.id} · {MODE_LABEL[selected.mode]}
            {paused && " · paused"}
          </>
        ) : (
          "Centre"
        )}
      </Badge>
    </div>
  );
}
