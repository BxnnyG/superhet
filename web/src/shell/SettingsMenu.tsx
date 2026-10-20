import { useEffect, useRef, useState } from "react";
import { Field, IconButton, SegmentedControl, SettingsIcon } from "../ui";
import { type ThemePreference } from "../ui/theme";

interface Props {
  theme: ThemePreference;
  onTheme: (theme: ThemePreference) => void;
}

/** Settings popover (theme for now); the slot for future preferences. */
export function SettingsMenu({ theme, onTheme }: Props) {
  const [open, setOpen] = useState(false);
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
          <p className="text-xs text-fg-subtle">Saved in this browser.</p>
        </div>
      )}
    </div>
  );
}
