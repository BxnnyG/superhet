// Tiny window-event bridge for commands that act on a component that owns its own state (the
// frequency readout's typed entry, the settings popover). Keeps the shortcut and palette code free
// of refs into the component tree.

import { useEffect, useRef } from "react";

export const EDIT_FREQUENCY_EVENT = "superhet:edit-frequency";
export const OPEN_SETTINGS_EVENT = "superhet:open-settings";

export function emitAppEvent(name: string): void {
  window.dispatchEvent(new CustomEvent(name));
}

/** Run `fn` whenever the window event `name` fires while mounted. */
export function useAppEvent(name: string, fn: () => void): void {
  const ref = useRef(fn);
  useEffect(() => {
    ref.current = fn;
  });
  useEffect(() => {
    const on = () => ref.current();
    window.addEventListener(name, on);
    return () => window.removeEventListener(name, on);
  }, [name]);
}
