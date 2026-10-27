import { AudioButton } from "../audio/AudioControls";
import { IconButton, Kbd, Logo, PanelRightIcon, PlainButton, SearchIcon, Slider } from "../ui";
import type { ThemePreference } from "../ui/theme";
import { AccountMenu } from "./AccountMenu";
import { DeviceSwitcher } from "./DeviceSwitcher";
import type { InspectorState } from "./inspector";
import { SettingsMenu } from "./SettingsMenu";
import { PALETTE_KEY } from "./shortcuts";
import type { Receiver } from "./useReceiver";

interface Props {
  r: Receiver;
  theme: ThemePreference;
  onTheme: (t: ThemePreference) => void;
  onShortcuts: () => void;
  onPalette: () => void;
  inspector: InspectorState;
}

/**
 * Top bar (no frequencies): logo, device switcher, command palette trigger, audio on/off with the
 * master volume, inspector toggle, settings and the account menu (avatar, name, role). Below 600 px only logo,
 * palette, audio, settings and the avatar.
 */
export function TopBar({ r, theme, onTheme, onShortcuts, onPalette, inspector }: Props) {
  return (
    <header className="flex h-12 shrink-0 items-center gap-2 border-b border-border bg-surface-1 px-3 shell:gap-3 shell:px-4">
      <Logo />
      <div className="max-phone:hidden">
        <DeviceSwitcher r={r} />
      </div>
      <div className="flex-1" />
      <PlainButton
        title={`Command palette (${PALETTE_KEY} or /)`}
        aria-haspopup="dialog"
        onClick={onPalette}
        className="inline-flex h-8 items-center gap-2 rounded-md border border-border-strong bg-surface-2 px-2.5 text-xs text-fg-subtle transition-colors duration-150 hover:text-fg shell:w-64 max-phone:size-11 max-phone:justify-center max-phone:px-0"
      >
        <SearchIcon size={14} className="shrink-0" />
        <span className="flex-1 truncate text-left max-phone:sr-only">Search or jump…</span>
        <span className="max-shell:hidden">
          <Kbd>{PALETTE_KEY}</Kbd>
        </span>
      </PlainButton>
      <div className="flex items-center gap-2">
        <AudioButton audio={r.audio} snapshot={r.audioSnap} className="max-phone:h-11" />
        <Slider
          className="w-24 max-phone:hidden"
          label="Master volume"
          hideLabel
          min={0}
          max={100}
          value={Math.round(r.audioSnap.master * 100)}
          valueLabel={`${Math.round(r.audioSnap.master * 100)} %`}
          title="Master volume"
          onChange={(v) => r.audio.setMasterVolume(v / 100)}
        />
      </div>
      <div className="flex items-center gap-1">
        <div className="max-phone:hidden">
          <IconButton
            label={inspector.open ? "Hide inspector" : "Show inspector"}
            tip={inspector.open ? "Hide inspector (])" : "Show inspector (])"}
            aria-controls="inspector"
            aria-expanded={inspector.open}
            onClick={inspector.toggle}
          >
            <PanelRightIcon />
          </IconButton>
        </div>
        <SettingsMenu theme={theme} onTheme={onTheme} onShortcuts={onShortcuts} />
      </div>
      <AccountMenu />
    </header>
  );
}
