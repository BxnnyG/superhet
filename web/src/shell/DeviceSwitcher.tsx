import { useDevices } from "../devices/context";
import type { DeviceInfo } from "../devices/api";
import type { SocketStatus } from "../spectrum/socket";
import { formatRate } from "../spectrum/TuningControls";
import { Badge, ChevronDownIcon, PlainButton, Popover, RowButton, cn, type BadgeTone } from "../ui";
import type { Receiver } from "./useReceiver";

const STATUS: Record<
  SocketStatus,
  { label: string; tone: BadgeTone; pulse: boolean; dot: string }
> = {
  connecting: { label: "Connecting", tone: "warn", pulse: true, dot: "bg-warn" },
  connected: { label: "Connected", tone: "success", pulse: false, dot: "bg-success" },
  reconnecting: { label: "Reconnecting", tone: "danger", pulse: true, dot: "bg-danger" },
};

function describe(d: DeviceInfo): string {
  return [d.tuner ?? d.driver, d.sampleRate ? formatRate(d.sampleRate) : null]
    .filter(Boolean)
    .join(" · ");
}

/**
 * Device pill of the top bar: connection dot, device name, driver and sample rate. Its popover
 * details the active device and, when the server knows several (one daemon per SDR), lists them
 * for switching: the whole receiver (spectrum, VFOs, audio, plugins) follows the choice.
 */
export function DeviceSwitcher({ r }: { r: Receiver }) {
  const { devices, select } = useDevices();
  const st = STATUS[r.status];
  const display = r.tuningSnap.display;
  const driver = display ? (display.tuner ?? display.driver) : null;
  const current = devices.find((d) => d.id === r.device);
  const name = current?.label ?? r.device;
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
          <span className={cn("text-fg", !current?.label && "font-mono")}>{name}</span>
          {driver && <span className="max-sm:hidden">{driver}</span>}
          {display && <span className="num max-sm:hidden">{formatRate(display.sampleRate)}</span>}
          <ChevronDownIcon size={12} className="shrink-0" />
        </PlainButton>
      )}
    >
      {(close) => (
        <div className="flex flex-col gap-2 text-sm">
          {devices.length > 1 && (
            <ul aria-label="Devices" className="flex flex-col gap-1">
              {devices.map((d) => (
                <li key={d.id}>
                  <RowButton
                    selected={d.id === r.device}
                    className="h-auto w-full py-1.5"
                    onClick={() => {
                      if (d.id !== r.device) select(d.id);
                      close();
                    }}
                  >
                    <span
                      aria-hidden="true"
                      className={cn(
                        "size-2 shrink-0 rounded-full",
                        d.online ? "bg-success" : "bg-fg-subtle",
                      )}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-fg">{d.label ?? d.id}</span>
                      <span className="block truncate text-xs text-fg-subtle">
                        {d.label ? `${d.id} · ` : ""}
                        {describe(d) || "no state"}
                      </span>
                    </span>
                    {!d.canControl && (
                      <Badge tone="neutral" title="You can look and listen, not change it">
                        Read-only
                      </Badge>
                    )}
                    {!d.online && <Badge tone="neutral">Offline</Badge>}
                  </RowButton>
                </li>
              ))}
            </ul>
          )}
          <div className="flex items-center justify-between gap-2">
            <span className="font-medium">
              {name}
              {current?.label && (
                <span className="ml-2 font-mono text-xs text-fg-subtle">{r.device}</span>
              )}
            </span>
            <span className="flex items-center gap-1.5">
              {current && !current.canControl && (
                <Badge tone="neutral" title="You can look and listen, not change it">
                  Read-only
                </Badge>
              )}
              <Badge tone={st.tone} dot pulse={st.pulse}>
                {st.label}
              </Badge>
            </span>
          </div>
          <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs">
            <dt className="text-fg-subtle">Driver</dt>
            <dd className="text-fg-muted">{display?.driver ?? "–"}</dd>
            <dt className="text-fg-subtle">Tuner</dt>
            <dd className="text-fg-muted">{display?.tuner ?? "–"}</dd>
            <dt className="text-fg-subtle">Sample rate</dt>
            <dd className="text-fg-muted num">{display ? formatRate(display.sampleRate) : "–"}</dd>
          </dl>
          {devices.length <= 1 && (
            <p className="text-xs text-fg-subtle">
              One daemon serves one SDR; start another to list more devices here.
            </p>
          )}
        </div>
      )}
    </Popover>
  );
}
