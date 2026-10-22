import type { LockHolder } from "../spectrum/api";
import { holdsUserLock, lockedBy, onlyUserLock } from "../state/locks";
import { Badge, IconButton, LockIcon, UnlockIcon } from "../ui";

interface Props {
  holders: readonly LockHolder[];
  /** A lock/unlock request is running, or the state is not known yet. */
  disabled?: boolean;
  /** Take (`true`) or drop (`false`) the user lock. */
  onToggle: (on: boolean) => void;
}

/**
 * Padlock next to the frequency readout. Pressed = the user lock is held. Other holders
 * (recordings, decoders) are named in the badge; they cannot be released from here.
 */
export function TuningLock({ holders, disabled, onToggle }: Props) {
  const locked = holders.length > 0;
  const mine = holdsUserLock(holders);
  const tip = !locked
    ? "Lock tuning: keep the centre and sample rate fixed"
    : mine
      ? onlyUserLock(holders)
        ? "Tuning locked by you. Click to unlock"
        : `${lockedBy(holders)}. Click to drop your lock`
      : `${lockedBy(holders)}. It is released when they finish`;
  return (
    <div className="flex items-center gap-1.5">
      <IconButton
        label={mine ? "Unlock tuning" : "Lock tuning"}
        tip={tip}
        variant="secondary"
        pressed={mine}
        disabled={disabled}
        onClick={() => onToggle(!mine)}
      >
        {locked ? <LockIcon /> : <UnlockIcon />}
      </IconButton>
      {locked && (
        <Badge tone="warn" title={`Centre and sample rate cannot change. ${tip}.`}>
          {lockedBy(holders)}
        </Badge>
      )}
    </div>
  );
}
