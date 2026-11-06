import { useEffect, useState } from "react";
import { Alert, Badge, cn } from "../ui";
import {
  channelLabel,
  encryptionLabel,
  formatDuration,
  protocolLabel,
  sourceLabel,
  targetLabel,
  type DsdCall,
  type DsdStatus,
} from "./model";

interface Props {
  status: DsdStatus;
  /** The status poll failed lately: do not claim a call is still going on. */
  stale: boolean;
  /** The decoder is running and gets input; false shows "waiting" instead of "idle". */
  running: boolean;
}

/** Milliseconds a call has been on the air, ticking while it is. */
function useElapsed(startedMs: number | null): number | null {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (startedMs === null) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [startedMs]);
  return startedMs === null ? null : Math.max(0, now - startedMs);
}

/** The explanation shown while dsd-fme is not installed. */
export function NeedsTool({ status }: { status: DsdStatus }) {
  const { tool } = status;
  return (
    <div data-testid="dsd-needs-tool">
      <Alert tone="warn">
        <p className="font-medium">Needs dsd-fme</p>
        <p className="mt-1">
          {tool.message ??
            "dsd-fme is not installed. Superhet does not ship it (it carries the AMBE vocoder); install it yourself."}
        </p>
        <p className="mt-1">
          Put the binary on the plugin host&apos;s <code>PATH</code> or point{" "}
          <code>SUPERHET_DSD_FME_PATH</code> at it, then restart this decoder. Nothing is downloaded
          automatically. Install steps, the legal notes and the limits are in{" "}
          <code>docs/plugins-dsd.md</code>.
        </p>
        {tool.searched.length > 0 && (
          <p className="mt-1 text-xs break-words">Looked for: {tool.searched.join("; ")}</p>
        )}
      </Alert>
    </div>
  );
}

/**
 * "Now talking" for DMR / dPMR / NXDN: the calls on the air as the decoder sees them (a DMR
 * channel can carry two, one per slot) with a running clock; idle shows the last call. While
 * dsd-fme is missing it explains what to install instead.
 */
export function DsdNow({ status, stale, running }: Props) {
  if (!stale && (status.tool.state === "missing" || status.tool.state === "failed")) {
    return status.tool.state === "missing" ? (
      <NeedsTool status={status} />
    ) : (
      <div data-testid="dsd-tool-failed">
        <Alert tone="danger">
          <p className="font-medium">dsd-fme could not be started</p>
          <p className="mt-1 text-xs break-words">{status.tool.message ?? status.tool.path}</p>
        </Alert>
      </div>
    );
  }
  const calls = stale ? [] : status.now;
  return (
    <div
      data-testid="dsd-now"
      className={cn(
        "flex flex-col gap-1 border-t border-border px-3 py-2 text-sm",
        calls.length > 0 && "bg-success-soft/40",
      )}
    >
      {calls.length === 0 ? (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <Badge tone="neutral" dot>
            {stale ? "Status unavailable" : running ? "Channel idle" : "Waiting for the decoder"}
          </Badge>
          {status.last !== null && !stale && (
            <span className="min-w-0 truncate text-fg-muted">
              Last: <span className="font-mono text-fg">{sourceLabel(status.last)}</span> →{" "}
              <span className="font-mono">{targetLabel(status.last)}</span>
              {status.last.durationMs !== null && ` · ${formatDuration(status.last.durationMs)}`}
            </span>
          )}
        </div>
      ) : (
        calls.map((call) => <Talking key={call.id} call={call} />)
      )}
    </div>
  );
}

function Talking({ call }: { call: DsdCall }) {
  const elapsed = useElapsed(call.startedMs);
  const channel = channelLabel(call);
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1" data-testid="dsd-now-call">
      <Badge tone={call.encrypted ? "warn" : "success"} dot pulse>
        On air
      </Badge>
      <Badge tone="neutral">{protocolLabel(call.mode)}</Badge>
      <span className="font-mono text-base font-semibold text-fg" data-testid="dsd-now-src">
        {sourceLabel(call)}
      </span>
      <span className="text-fg-muted">→</span>
      <span className="font-mono text-fg">{targetLabel(call)}</span>
      {channel !== "" && <span className="num text-xs text-fg-muted">{channel}</span>}
      {call.encrypted && (
        <Badge tone="warn" title="The audio of encrypted calls is not played">
          {encryptionLabel(call)}
          {call.muted ? " · muted" : ""}
        </Badge>
      )}
      {elapsed !== null && (
        <span className="num text-xs text-fg-subtle">{formatDuration(elapsed)}</span>
      )}
    </div>
  );
}
