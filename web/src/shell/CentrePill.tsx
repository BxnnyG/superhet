import { formatFrequency } from "../spectrum/freq";
import { AlertIcon, LockIcon, PlainButton, Popover, SettingsIcon, cn } from "../ui";
import { overloadNotice } from "./overload";
import { SourceSettings } from "./SourceSettings";
import type { Receiver } from "./useReceiver";

/**
 * Compact secondary readout of the device centre ("Centre 145.000 · lock"). It opens a popover
 * with the source settings; the VFO frequency is the only big number on screen.
 */
export function CentrePill({ r }: { r: Receiver }) {
  const centerHz = r.tuningSnap.display?.centerHz ?? null;
  const locked = r.locks.length > 0;
  const overload = overloadNotice(r.tuningSnap.state);
  return (
    <Popover
      label="Source settings"
      align="end"
      trigger={({ props }) => (
        <PlainButton
          {...props}
          title={
            overload
              ? `${overload.summary} Open the source settings to lower the gain.`
              : "Device centre and source settings"
          }
          aria-label={`Centre ${centerHz === null ? "unknown" : formatFrequency(centerHz).slice(0, -4)}${locked ? ", locked" : ""}${overload ? ", ADC overload, reduce gain" : ""}, source settings`}
          className={cn(
            "inline-flex h-8 items-center gap-2 rounded-full border border-border-strong",
            "bg-surface-1 px-3 text-xs text-fg-muted transition-colors duration-150",
            "hover:bg-surface-2 hover:text-fg",
          )}
        >
          <SettingsIcon size={14} className="shrink-0" />
          <span>Centre</span>
          <span className="font-mono text-sm text-fg num">
            {centerHz === null ? "–" : formatFrequency(centerHz).slice(0, -4)}
          </span>
          {locked && <LockIcon size={12} className="shrink-0 text-warn" />}
          {overload && (
            <span
              data-testid="overload-chip"
              className="inline-flex h-5 items-center gap-1 rounded-full bg-warn-soft px-1.5 text-[11px] font-medium text-warn ring-1 ring-warn/20 ring-inset"
            >
              <AlertIcon size={11} className="shrink-0" />
              Overload
            </span>
          )}
        </PlainButton>
      )}
    >
      <h2 className="mb-3 text-xs font-semibold tracking-wider text-fg-muted uppercase">Source</h2>
      <SourceSettings r={r} />
    </Popover>
  );
}
