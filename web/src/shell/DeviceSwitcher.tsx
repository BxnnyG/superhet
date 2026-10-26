import type { SocketStatus } from "../spectrum/socket";
import { formatRate } from "../spectrum/TuningControls";
import { Badge, ChevronDownIcon, PlainButton, Popover, cn, type BadgeTone } from "../ui";
import type { Receiver } from "./useReceiver";

const STATUS: Record<
  SocketStatus,
  { label: string; tone: BadgeTone; pulse: boolean; dot: string }
> = {
  connecting: { label: "Connecting", tone: "warn", pulse: true, dot: "bg-warn" },
  connected: { label: "Connected", tone: "success", pulse: false, dot: "bg-success" },
  reconnecting: { label: "Reconnecting", tone: "danger", pulse: true, dot: "bg-danger" },
};

/**
 * Device pill of the top bar: connection dot, device id, driver and sample rate. Its popover
 * details the active device; one daemon serves one SDR, so this is a single-entry switcher until
 * several devices can be listed.
 */
export function DeviceSwitcher({ r }: { r: Receiver }) {
  const st = STATUS[r.status];
  const display = r.tuningSnap.display;
  const driver = display ? (display.tuner ?? display.driver) : null;
  return (
    <Popover
      label="Device"
      align="start"
      panelClassName="w-72"
      trigger={({ props }) => (
        <PlainButton
          {...props}
          title="Active device"
          className="inline-flex h-8 items-center gap-2 rounded-md border border-transparent px-2 text-xs text-fg-muted transition-colors duration-150 hover:bg-surface-2 hover:text-fg"
        >
          <span
            role="status"
            aria-label={st.label}
            className={cn("size-2 shrink-0 rounded-full", st.dot, st.pulse && "animate-pulse")}
          />
          <span className="font-mono text-fg">{r.device}</span>
          {driver && <span className="max-sm:hidden">{driver}</span>}
          {display && <span className="num max-sm:hidden">{formatRate(display.sampleRate)}</span>}
          <ChevronDownIcon size={12} className="shrink-0" />
        </PlainButton>
      )}
    >
      <div className="flex flex-col gap-2 text-sm">
        <div className="flex items-center justify-between gap-2">
          <span className="font-mono font-medium">{r.device}</span>
          <Badge tone={st.tone} dot pulse={st.pulse}>
            {st.label}
          </Badge>
        </div>
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
          <dt className="text-fg-subtle">Driver</dt>
          <dd className="text-fg-muted">{display?.driver ?? "–"}</dd>
          <dt className="text-fg-subtle">Tuner</dt>
          <dd className="text-fg-muted">{display?.tuner ?? "–"}</dd>
          <dt className="text-fg-subtle">Sample rate</dt>
          <dd className="text-fg-muted num">{display ? formatRate(display.sampleRate) : "–"}</dd>
        </dl>
        <p className="text-xs text-fg-subtle">This daemon serves one device.</p>
      </div>
    </Popover>
  );
}
