import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { ChangePasswordDialog } from "../auth/ChangePasswordDialog";
import { useAuth } from "../auth/context";
import {
  Button,
  Field,
  IconButton,
  KeyIcon,
  Kbd,
  LogOutIcon,
  SegmentedControl,
  SettingsIcon,
  UserIcon,
} from "../ui";
import { type ThemePreference } from "../ui/theme";
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
  const [passwordOpen, setPasswordOpen] = useState(false);
  const auth = useAuth();
  const rootRef = useRef<HTMLDivElement>(null);

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
            Zoom: the wheel zooms everywhere, Shift+wheel tunes the active VFO. Tune: the wheel
            tunes the active VFO; Ctrl+wheel zooms. Saved in this browser.
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
          {auth && (
            <div className="flex flex-col gap-1 border-t border-border pt-3">
              <div className="flex min-w-0 items-center gap-2 px-1 pb-1 text-sm">
                <UserIcon className="shrink-0 text-fg-subtle" />
                <span className="sr-only">Signed in as</span>
                <span className="truncate font-medium" data-testid="signed-in-user">
                  {auth.username}
                </span>
              </div>
              <Button
                variant="ghost"
                icon={<KeyIcon />}
                className="justify-start"
                onClick={() => {
                  setOpen(false);
                  setPasswordOpen(true);
                }}
              >
                Change password
              </Button>
              <Button
                variant="ghost"
                icon={<LogOutIcon />}
                className="justify-start"
                onClick={() => {
                  setOpen(false);
                  void auth.signOut();
                }}
              >
                Sign out
              </Button>
            </div>
          )}
        </div>
      )}
      {passwordOpen && auth && (
        <ChangePasswordDialog client={auth.client} onClose={() => setPasswordOpen(false)} />
      )}
    </div>
  );
}
