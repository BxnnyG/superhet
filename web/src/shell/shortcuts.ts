// Keyboard shortcut map: which key does what, and when keys must be left alone (typing in a
// field, a focused slider, an open dialog, browser shortcuts). Pure; the effects are applied by
// runShortcut() in useShortcuts.ts.

import type { VfoMode } from "../vfo/api";

export type ShortcutAction =
  /** Tune the active VFO by this many steps (negative = down). */
  | { type: "step"; steps: number }
  /** Select the next (+1) or previous (-1) VFO. */
  | { type: "vfo"; delta: 1 | -1 }
  /** Set the mode of the active VFO. */
  | { type: "mode"; mode: VfoMode; label: string }
  | { type: "listen" }
  | { type: "autoSquelch" }
  | { type: "bookmark" }
  /** Zoom factor around the display centre (< 1 zooms in). */
  | { type: "zoom"; factor: number }
  | { type: "zoomReset" }
  | { type: "newVfo" }
  | { type: "delete" }
  /** Show / hide the inspector. */
  | { type: "inspector" }
  | { type: "help" };

/** Modes on keys 1-6. */
export const MODE_KEYS: readonly { mode: VfoMode; label: string }[] = [
  { mode: "wfm", label: "WFM" },
  { mode: "nfm", label: "NFM" },
  { mode: "am", label: "AM" },
  { mode: "usb", label: "USB" },
  { mode: "lsb", label: "LSB" },
  { mode: "cw", label: "CW" },
];

/** Steps of Shift+Arrow relative to a plain arrow. */
export const BIG_STEP_FACTOR = 10;
export const ZOOM_IN_FACTOR = 0.5;
export const ZOOM_OUT_FACTOR = 2;

export interface KeyLike {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** The action for a key press, or null when the key is not a shortcut. */
export function shortcutFor(e: KeyLike): ShortcutAction | null {
  if (e.ctrlKey || e.altKey || e.metaKey) return null;
  switch (e.key) {
    case "ArrowLeft":
      return { type: "step", steps: e.shiftKey ? -BIG_STEP_FACTOR : -1 };
    case "ArrowRight":
      return { type: "step", steps: e.shiftKey ? BIG_STEP_FACTOR : 1 };
    case "ArrowUp":
      return { type: "vfo", delta: 1 };
    case "ArrowDown":
      return { type: "vfo", delta: -1 };
    case "+":
    case "=":
      return { type: "zoom", factor: ZOOM_IN_FACTOR };
    case "-":
      return { type: "zoom", factor: ZOOM_OUT_FACTOR };
    case "0":
      return { type: "zoomReset" };
    case "Delete":
      return { type: "delete" };
    case "?":
      return { type: "help" };
    case "]":
      return { type: "inspector" };
  }
  if (/^[1-6]$/.test(e.key)) {
    const entry = MODE_KEYS[Number(e.key) - 1];
    return entry ? { type: "mode", ...entry } : null;
  }
  switch (e.key.toLowerCase()) {
    case "m":
      return { type: "listen" };
    case "s":
      return { type: "autoSquelch" };
    case "b":
      return { type: "bookmark" };
    case "n":
      return { type: "newVfo" };
  }
  return null;
}

const OWN_KEYS_ROLES = new Set([
  "slider",
  "spinbutton",
  "separator",
  "radio",
  "radiogroup",
  "tab",
  "tablist",
  "listbox",
  "option",
  "combobox",
  "textbox",
  "menu",
  "menuitem",
]);

/**
 * True when the key belongs to the focused element or an open dialog: text fields, selects,
 * widgets with their own arrow keys (sliders, the splitter, digit readout) and anything inside a
 * modal dialog.
 */
export function ignoresShortcuts(target: EventTarget | null, root: ParentNode = document): boolean {
  if (root.querySelector('[aria-modal="true"]')) return true;
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  const role = target.getAttribute("role");
  return role !== null && OWN_KEYS_ROLES.has(role);
}

/** Documentation of the map for the shortcuts dialog; keep in step with shortcutFor(). */
export const SHORTCUT_GROUPS: readonly {
  title: string;
  items: readonly { keys: readonly string[]; label: string }[];
}[] = [
  {
    title: "Tuning",
    items: [
      { keys: ["←", "→"], label: "Step the active VFO down / up" },
      { keys: ["Shift", "←/→"], label: "Ten steps at once" },
      { keys: ["↑", "↓"], label: "Next / previous VFO" },
      { keys: ["N"], label: "New VFO at the display centre" },
      { keys: ["Delete"], label: "Remove the active VFO (undo from the notice)" },
      { keys: ["B"], label: "Bookmark the active VFO" },
    ],
  },
  {
    title: "Receiver",
    items: [
      { keys: ["1", "2", "3"], label: "Mode WFM / NFM / AM" },
      {
        keys: ["4", "5", "6"],
        label: "Mode USB / LSB / CW (bandwidth resets to the mode default)",
      },
      { keys: ["M"], label: "Listen to the active VFO on / off" },
      { keys: ["S"], label: "Squelch Auto on / off" },
    ],
  },
  {
    title: "Spectrum",
    items: [
      { keys: ["+", "−"], label: "Zoom in / out" },
      { keys: ["0"], label: "Reset zoom" },
      { keys: ["Wheel"], label: "Zoom around the pointer (also over a VFO)" },
      { keys: ["Shift", "Wheel"], label: "Tune the active VFO by one step per notch" },
    ],
  },
  {
    title: "Layout",
    items: [
      { keys: ["]"], label: "Show / hide the inspector" },
      { keys: ["?"], label: "Show this list" },
    ],
  },
];
