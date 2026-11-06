import { useEffect, useState } from "react";
import { formatCoords, mapLink } from "../decoders/render";
import { Badge, cn } from "../ui";
import { encryptionLabel, formatDuration, type M17Call, type M17Status } from "./model";

interface Props {
  status: M17Status;
  /** The status poll failed lately: do not claim a call is still going on. */
  stale: boolean;
  /** The decoder is running and gets input; false shows "waiting" instead of "idle". */
  running: boolean;
}

/** Seconds a call has been on the air, ticking while it is. */
function useElapsed(startedMs: number | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (startedMs === null) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [startedMs]);
  return startedMs === null ? null : Math.max(0, now - startedMs);
}

/**
 * "Now talking": the station on the air as the decoder sees it (callsign, destination, CAN,
 * encryption, position) with a running clock; idle shows the last call.
 */
export function NowTalking({ status, stale, running }: Props) {
  const call = stale ? null : status.now;
  const elapsed = useElapsed(call?.startedMs ?? null);
  return (
    <div
      data-testid="m17-now"
      className={cn(
        "flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-border px-3 py-2 text-sm",
        call !== null && "bg-success-soft/40",
      )}
    >
      {call === null ? (
        <>
          <Badge tone="neutral" dot>
            {stale ? "Status unavailable" : running ? "Channel idle" : "Waiting for the decoder"}
          </Badge>
          {status.last !== null && !stale && (
            <span className="min-w-0 truncate text-fg-muted">
              Last: <span className="font-mono text-fg">{status.last.src}</span> →{" "}
              <span className="font-mono">{status.last.dst}</span>
              {status.last.durationMs !== null && ` · ${formatDuration(status.last.durationMs)}`}
            </span>
          )}
        </>
      ) : (
        <Talking call={call} elapsed={elapsed} />
      )}
    </div>
  );
}

function Talking({ call, elapsed }: { call: M17Call; elapsed: number | null }) {
  const crypto = encryptionLabel(call);
  const coords = call.position === null ? null : formatCoords(call.position.lat, call.position.lon);
  const link = call.position === null ? null : mapLink(call.position.lat, call.position.lon);
  return (
    <>
      <Badge tone={call.encrypted === true ? "warn" : "success"} dot pulse>
        On air
      </Badge>
      <span className="font-mono text-base font-semibold text-fg" data-testid="m17-now-src">
        {call.src}
      </span>
      <span className="text-fg-muted">→</span>
      <span className="font-mono text-fg">{call.dst}</span>
      <Badge tone="neutral">CAN {call.can}</Badge>
      {crypto !== null && call.encrypted === true && (
        <Badge tone="warn" title="The audio of encrypted calls is not played">
          {crypto}
          {call.muted ? " · muted" : ""}
        </Badge>
      )}
      {call.mode !== "voice" && <Badge tone="neutral">{call.mode}</Badge>}
      {elapsed !== null && (
        <span className="num text-xs text-fg-subtle">{formatDuration(elapsed)}</span>
      )}
      {call.text !== null && <span className="min-w-0 truncate text-fg-muted">“{call.text}”</span>}
      {coords !== null && (
        <span className="num text-xs text-fg-muted">
          {coords}
          {link !== null && (
            <>
              {" "}
              <a
                href={link.href}
                target="_blank"
                rel="noopener noreferrer"
                className="text-accent underline-offset-2 hover:underline"
              >
                {link.label}
              </a>
            </>
          )}
        </span>
      )}
    </>
  );
}
