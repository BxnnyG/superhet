import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button, Field, IconButton, Kbd, SegmentedControl, SettingsIcon } from "../ui";
import { type ThemePreference } from "../ui/theme";
import { OPEN_SETTINGS_EVENT, useAppEvent } from "./appEvents";
import { AccountDialogs, AccountSection, type AccountDialogKind } from "./AccountSection";
import { getWheelMode, setWheelMode, subscribeWheelMode } from "../spectrum/wheelMode";

interface Props {
  theme: ThemePreference;
  onTheme: (theme: ThemePreference) => void;
  /** Open the keyboard shortcuts dialog. */
  onShortcuts: () => void;
}

/** Settings popover (theme for now); the slot for future preferences. */
export function SettingsMenu({ theme, onTheme, onShortcuts }: Props) {
  const wheel = useSyncExternalStore(subscribeWheelMode, getWheelMode);
  const [open, setOpen] = useState(false);
  const [accountDialog, setAccountDialog] = useState<AccountDialogKind | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  useAppEvent(OPEN_SETTINGS_EVENT, () => setOpen(true));

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) {
        setOpen(false);
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      <IconButton
        label="Settings"
        className="max-phone:size-11"
        tooltipAlign="end"
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen((o) => !o)}
      >
        <SettingsIcon />
      </IconButton>
      {open && (
        <div
          role="dialog"
          aria-label="Settings"
          className="absolute top-full right-0 z-50 mt-2 flex w-64 flex-col gap-3 rounded-lg border border-border bg-surface-1 p-3 shadow-pop"
        >
          <Field label="Theme">
            <SegmentedControl
              aria-label="Theme"
              value={theme}
              onChange={onTheme}
              className="w-full"
              options={[
                { value: "system", label: "System" },
                { value: "light", label: "Light" },
                { value: "dark", label: "Dark" },
              ]}
            />
          </Field>
          <Field label="Mouse wheel">
            <SegmentedControl
              aria-label="Mouse wheel"
              value={wheel}
              onChange={setWheelMode}
              className="w-full"
              options={[
                { value: "zoom", label: "Zoom" },
                { value: "tune", label: "Tune" },
              ]}
            />
          </Field>
          <p className="text-xs text-fg-subtle">
            Zoom: the wheel zooms everywhere, Shift+wheel over the active passband changes its
            bandwidth, Shift or Alt+wheel elsewhere tunes the active VFO. Tune: the wheel tunes the
            active VFO; Ctrl+wheel zooms. Saved in this browser.
          </p>
          <Button
            variant="ghost"
            className="justify-between"
            onClick={() => {
              setOpen(false);
              onShortcuts();
            }}
          >
            Keyboard shortcuts
            <Kbd>?</Kbd>
          </Button>
          <AccountSection onClose={() => setOpen(false)} onDialog={setAccountDialog} />
        </div>
      )}
      <AccountDialogs kind={accountDialog} onClose={() => setAccountDialog(null)} />
    </div>
  );
}
