import { Badge, FrequencyReadout } from "../ui";

interface Props {
  /** Confirmed/optimistic device centre; null while unknown. */
  centerHz: number | null;
  onCenter: (hz: number) => void;
  /** Why the centre cannot be changed (tuning lock), or null. */
  centerLockedReason?: string | null;
}

/** Big readout of the device centre frequency (VFOs are tuned in the receiver dock). */
export function ReceiverFrequency({ centerHz, onCenter, centerLockedReason = null }: Props) {
  return (
    <div className="flex items-center gap-3">
      <FrequencyReadout
        label="Centre frequency"
        valueHz={centerHz}
        onChange={onCenter}
        locked={centerLockedReason !== null}
        lockedReason={centerLockedReason ?? undefined}
      />
      <Badge title="The readout tunes the device centre; VFOs are tuned in the receiver dock below">
        Centre
      </Badge>
    </div>
  );
}
