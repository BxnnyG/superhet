import type { SocketStatus } from "../spectrum/socket";
import { formatRate } from "../spectrum/TuningControls";
import { Badge, IconButton, Logo, MenuIcon, type BadgeTone } from "../ui";
import type { ThemePreference } from "../ui/theme";
import { lockedBy } from "../state/locks";
import { ReceiverFrequency } from "./ReceiverFrequency";
import { SettingsMenu } from "./SettingsMenu";
import { TuningLock } from "./TuningLock";
import type { Receiver } from "./useReceiver";

const STATUS: Record<SocketStatus, { label: string; tone: BadgeTone; pulse: boolean }> = {
  connecting: { label: "Connecting", tone: "warn", pulse: true },
  connected: { label: "Connected", tone: "success", pulse: false },
  reconnecting: { label: "Reconnecting", tone: "danger", pulse: true },
};

interface Props {
  r: Receiver;
  theme: ThemePreference;
  onTheme: (t: ThemePreference) => void;
  drawerOpen: boolean;
  onMenu: () => void;
}

export function TopBar({ r, theme, onTheme, drawerOpen, onMenu }: Props) {
  const st = STATUS[r.status];
  const display = r.tuningSnap.display;
  return (
    <header className="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-surface-1 px-3 py-2 shell:px-4">
      <div className="shell:hidden">
        <IconButton
          label="Open controls"
          aria-expanded={drawerOpen}
          aria-controls="sidebar"
          onClick={onMenu}
        >
          <MenuIcon />
        </IconButton>
      </div>
      <Logo />
      <div role="status" className="flex items-center gap-2">
        <Badge tone={st.tone} dot pulse={st.pulse}>
          {st.label}
        </Badge>
        <span className="hidden text-xs text-fg-muted sm:inline">
          <span className="font-mono">{r.device}</span>
          {display && (
            <>
              {" · "}
              {display.tuner ?? display.driver}
              {" · "}
              <span className="num">{formatRate(display.sampleRate)}</span>
            </>
          )}
        </span>
      </div>
      <div className="order-last flex basis-full flex-wrap items-center justify-center gap-x-3 gap-y-1 shell:order-none shell:basis-auto shell:flex-1">
        <ReceiverFrequency
          vfo={r.selected}
          step={r.step}
          onStep={r.setStep}
          onVfo={(id, patch) => r.vfoCtl.update(id, patch)}
          centerHz={display?.centerHz ?? null}
          onCenter={(hz) => r.tuning.request({ centerHz: hz })}
          centerLockedReason={
            r.locks.length > 0 ? `${lockedBy(r.locks)}: tune the VFO instead, or unlock` : null
          }
          locked={r.locks.length > 0}
          lock={
            <TuningLock
              holders={r.locks}
              disabled={!r.tuningSnap.state || r.tuningSnap.lockBusy}
              onToggle={r.toggleLock}
            />
          }
        />
      </div>
      <div className="ml-auto flex items-center gap-2 shell:ml-0">
        <SettingsMenu theme={theme} onTheme={onTheme} />
      </div>
    </header>
  );
}
