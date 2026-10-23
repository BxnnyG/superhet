import type { RdsState } from "../audio/rds";
import { ptyName } from "../audio/rds";
import { Badge } from "../ui";

interface Props {
  /** Latest RDS state of the VFO; undefined while nothing was decoded. */
  rds: RdsState | undefined;
  /** Effective de-emphasis of the VFO (75 µs selects the North American PTY names). */
  deemphasis: string | null;
}

/**
 * Station information decoded from RDS: service name, programme type, RadioText and the PI code.
 * The row keeps its height when there is no data so the dock does not jump.
 */
export function RdsPanel({ rds, deemphasis }: Props) {
  const hasData = rds !== undefined;
  return (
    <div
      role="group"
      aria-label="RDS"
      className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-1 px-3 py-2"
    >
      <span className="text-[11px] font-medium tracking-wide text-fg-subtle uppercase">RDS</span>
      {!hasData ? (
        <span className="text-sm text-fg-subtle">no data</span>
      ) : (
        <>
          <span
            data-testid="rds-ps"
            title="Programme service name"
            className="min-w-[8ch] font-mono text-base font-semibold tracking-wider whitespace-pre text-fg"
          >
            {rds.ps ?? "········"}
          </span>
          {rds.pty !== null && rds.pty > 0 && (
            <Badge tone="accent" title={`Programme type ${rds.pty}`}>
              {ptyName(rds.pty, deemphasis)}
            </Badge>
          )}
          {rds.tp === true && (
            <Badge tone="neutral" title="Traffic programme">
              TP
            </Badge>
          )}
          {rds.ta === true && (
            <Badge tone="warn" title="Traffic announcement in progress">
              TA
            </Badge>
          )}
          {rds.pi !== null && (
            <span title="Programme identification" className="font-mono text-xs text-fg-muted num">
              PI {rds.pi}
            </span>
          )}
          {rds.rt !== null && (
            <span
              data-testid="rds-rt"
              title={rds.rt}
              className="min-w-0 flex-1 basis-64 truncate text-sm text-fg-muted"
            >
              {rds.rt}
            </span>
          )}
          {!rds.synced && (
            <span className="text-xs text-fg-subtle" title="Data is no longer being received">
              signal lost
            </span>
          )}
        </>
      )}
    </div>
  );
}
