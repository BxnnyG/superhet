import { usePermissions } from "../auth/context";
import { NEEDS_OPERATOR } from "../auth/permissions";
import type { LockHolder } from "../spectrum/api";
import { foreignUserLock, holdsUserLock, lockedBy, onlyUserLock } from "../state/locks";
import { Badge, IconButton, LockIcon, UnlockIcon } from "../ui";

interface Props {
  holders: readonly LockHolder[];
  /** A lock/unlock request is running, or the state is not known yet. */
  disabled?: boolean;
  /** Take (`true`) or drop (`false`) the user lock; `force` (admins) drops other accounts' locks. */
  onToggle: (on: boolean, force?: boolean) => void;
}

/**
 * Padlock next to the frequency readout. Pressed = the viewer's own lock is held. The holder is
 * named in the badge ("Locked by alice"); an account releases only its own lock, an admin can
 * force-release the others. Recordings and decoders release theirs when they finish.
 */
export function TuningLock({ holders, disabled, onToggle }: Props) {
  const perms = usePermissions();
  const locked = holders.length > 0;
  const mine = holdsUserLock(holders);
  const foreign = foreignUserLock(holders);
  const force = !mine && foreign && perms.canForceUnlock;
  let tip: string;
  if (!perms.canLock) {
    tip = locked ? `${lockedBy(holders)}. ${NEEDS_OPERATOR} can change it` : NEEDS_OPERATOR;
  } else if (!locked) {
    tip = "Lock tuning: keep the centre and sample rate fixed";
  } else if (mine) {
    tip = onlyUserLock(holders)
      ? "Tuning locked by you. Click to unlock"
      : `${lockedBy(holders)}. Click to drop your lock`;
  } else if (force) {
    tip = `${lockedBy(holders)}. Click to force-release it (admin)`;
  } else if (foreign) {
    tip = `${lockedBy(holders)}. Only they or an admin can release it`;
  } else {
    tip = `${lockedBy(holders)}. It is released when they finish`;
  }
  const label = mine ? "Unlock tuning" : force ? "Force unlock tuning" : "Lock tuning";
  const blocked = !perms.canLock || (locked && !mine && !force);
  return (
    <div className="flex items-center gap-1.5">
      <IconButton
        label={label}
        tip={tip}
        variant="secondary"
        pressed={mine}
        disabled={disabled || blocked}
        onClick={() => (force ? onToggle(false, true) : onToggle(!mine))}
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
