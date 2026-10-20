import { AudioButton } from "../audio/AudioControls";
import type { SocketStatus } from "../spectrum/socket";
import { formatRate } from "../spectrum/TuningControls";
import { Badge, IconButton, Logo, MenuIcon, Slider, type BadgeTone } from "../ui";
import type { ThemePreference } from "../ui/theme";
import { ReceiverFrequency } from "./ReceiverFrequency";
import { SettingsMenu } from "./SettingsMenu";
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
  const sel = r.selected;
  const level = sel ? (r.audioSnap.levels[sel.id] ?? { volume: 0.8, muted: false }) : null;
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
      <div className="order-last flex basis-full justify-center shell:order-none shell:basis-auto shell:flex-1">
        <ReceiverFrequency
          centerHz={display?.centerHz ?? null}
          selected={r.selected}
          onCenter={(hz) => r.tuning.request({ centerHz: hz })}
          onVfo={(id, hz) => r.vfoCtl.update(id, { freqHz: hz })}
        />
      </div>
      <div className="ml-auto flex items-center gap-2 shell:ml-0">
        <Slider
          className="hidden w-24 sm:flex"
          label="Volume of the selected VFO"
          hideLabel
          min={0}
          max={100}
          disabled={!sel}
          value={level ? Math.round(level.volume * 100) : 0}
          valueLabel={sel ? `${Math.round((level?.volume ?? 0) * 100)} %` : "no VFO selected"}
          title={sel ? `Volume of VFO ${sel.id}` : "Select a VFO to set its volume"}
          onChange={(v) => sel && r.audio.setVolume(sel.id, v / 100)}
        />
        <AudioButton audio={r.audio} snapshot={r.audioSnap} />
        <SettingsMenu theme={theme} onTheme={onTheme} />
      </div>
    </header>
  );
}
