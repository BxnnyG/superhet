import { useEffect, useState } from "react";
import { Alert, Badge, cn } from "../ui";
import {
  cellEncryptionLabel,
  cellLabel,
  channelLabel,
  formatDuration,
  sourceLabel,
  targetLabel,
  type TetraCall,
  type TetraStatus,
} from "./model";

interface Props {
  status: TetraStatus;
  /** The status poll failed lately: do not claim a call is still going on. */
  stale: boolean;
  /** The decoder is running and gets input; false shows "waiting" instead of "searching". */
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

/** Shown while tetra-rx is not found (it is part of the image, so this is a broken install). */
export function NeedsTool({ status }: { status: TetraStatus }) {
  const { tool } = status;
  return (
    <div data-testid="tetra-needs-tool">
      <Alert tone="warn">
        <p className="font-medium">tetra-rx not found</p>
        <p className="mt-1">
          {tool.message ??
            "tetra-rx (osmo-tetra) is not installed. The Superhet image contains it; outside the image install it yourself."}
        </p>
        <p className="mt-1">
          Put it on the plugin host&apos;s <code>PATH</code> or point{" "}
          <code>SUPERHET_TETRA_RX_PATH</code> at it, then restart this decoder. Details are in{" "}
          <code>docs/plugins-tetra.md</code>.
        </p>
        {tool.searched.length > 0 && (
          <p className="mt-1 text-xs break-words">Looked for: {tool.searched.join("; ")}</p>
        )}
      </Alert>
    </div>
  );
}

/** Voice needs the user's ETSI codec: said once, next to the signalling that works without it. */
export function NeedsCodec({ status }: { status: TetraStatus }) {
  const { codec } = status;
  const failed = codec.state === "failed";
  return (
    <div data-testid="tetra-needs-codec">
      <Alert tone={failed ? "danger" : "info"}>
        <p className="font-medium">
          {failed ? "The ETSI codec stopped" : "Voice needs the ETSI codec"}
        </p>
        <p className="mt-1">
          {failed
            ? (codec.detail ?? "The codec programs ended; restart this decoder.")
            : "Cell, calls, short messages and encryption flags are decoded without it. To hear voice, install the ETSI TETRA speech codec yourself (it is not part of Superhet and is never downloaded by it) and point SUPERHET_TETRA_CODEC_DIR at the directory with cdecoder and sdecoder, then restart this decoder. Steps and legal notes are in docs/plugins-tetra.md."}
        </p>
        {codec.searched.length > 0 && (
          <p className="mt-1 text-xs break-words">Looked for: {codec.searched.join("; ")}</p>
        )}
      </Alert>
    </div>
  );
}

/**
 * "Now" for TETRA: whether a cell is in sync and which one, the calls on the air (source,
 * group, traffic channel, encryption, clock) and the last call. Explains what to install while the
 * decoder or the ETSI codec is missing.
 */
export function TetraNow({ status, stale, running }: Props) {
  if (!stale && (status.tool.state === "missing" || status.tool.state === "failed")) {
    return status.tool.state === "missing" ? (
      <NeedsTool status={status} />
    ) : (
      <div data-testid="tetra-tool-failed">
        <Alert tone="danger">
          <p className="font-medium">tetra-rx could not be started</p>
          <p className="mt-1 text-xs break-words">{status.tool.message ?? status.tool.path}</p>
        </Alert>
      </div>
    );
  }
  const calls = stale ? [] : status.now;
  const codecMissing =
    !stale && (status.codec.state === "missing" || status.codec.state === "failed");
  const cell = status.cell;
  const encryption = cell === null ? null : cellEncryptionLabel(cell);
  return (
    <div data-testid="tetra-now" className="border-t border-border">
      {codecMissing && (
        <div className="px-3 pt-2">
          <NeedsCodec status={status} />
        </div>
      )}
      <div
        className={cn(
          "flex flex-col gap-1 px-3 py-2 text-sm",
          calls.length > 0 && "bg-success-soft/40",
        )}
      >
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <Badge tone={stale ? "neutral" : status.sync ? "success" : "neutral"} dot>
            {stale
              ? "Status unavailable"
              : status.sync
                ? "Cell in sync"
                : running
                  ? "Searching for a cell"
                  : "Waiting for the decoder"}
          </Badge>
          {cell !== null && !stale && (
            <span className="num text-xs text-fg-muted" data-testid="tetra-cell">
              {cellLabel(cell)}
            </span>
          )}
          {encryption !== null && !stale && (
            <Badge tone="warn" title="Nothing is decrypted; encrypted calls are flagged only">
              {encryption}
            </Badge>
          )}
          {status.offsetHz !== null && status.sync && !stale && (
            <span
              className="num text-xs text-fg-subtle"
              title="Carrier offset the demodulator measured"
            >
              {status.offsetHz >= 0 ? "+" : ""}
              {status.offsetHz} Hz
            </span>
          )}
          {status.neighbours.length > 0 && !stale && (
            <span className="text-xs text-fg-subtle">
              {status.neighbours.length} neighbour cell{status.neighbours.length === 1 ? "" : "s"}
            </span>
          )}
        </div>
        {calls.length === 0
          ? status.last !== null &&
            !stale && (
              <span className="min-w-0 truncate text-fg-muted">
                Last: <span className="font-mono text-fg">{sourceLabel(status.last)}</span> →{" "}
                <span className="font-mono">{targetLabel(status.last)}</span>
                {status.last.durationMs !== null && ` · ${formatDuration(status.last.durationMs)}`}
              </span>
            )
          : calls.map((call) => <Talking key={call.id} call={call} />)}
      </div>
    </div>
  );
}

function Talking({ call }: { call: TetraCall }) {
  const elapsed = useElapsed(call.startedMs);
  const channel = channelLabel(call);
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-1" data-testid="tetra-now-call">
      <Badge tone={call.encrypted ? "warn" : "success"} dot pulse>
        On air
      </Badge>
      <span className="font-mono text-base font-semibold text-fg" data-testid="tetra-now-src">
        {call.talker !== null ? String(call.talker) : sourceLabel(call)}
      </span>
      <span className="text-fg-muted">→</span>
      <span className="font-mono text-fg">{targetLabel(call)}</span>
      {channel !== "" && <span className="num text-xs text-fg-muted">{channel}</span>}
      {call.encrypted && (
        <Badge tone="warn" title="The audio of encrypted calls is not played">
          encrypted{call.muted ? " · muted" : ""}
        </Badge>
      )}
      {elapsed !== null && (
        <span className="num text-xs text-fg-subtle">{formatDuration(elapsed)}</span>
      )}
    </div>
  );
}
