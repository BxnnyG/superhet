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
  /** Select the VFO at this zero-based position (Alt+1..9). */
  | { type: "vfoAt"; index: number }
  /** Type a frequency into the active VFO's readout. */
  | { type: "focusFrequency" }
  | { type: "palette" }
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
  /** Physical key; used for Alt+digit, where `key` depends on layout/OS (macOS). */
  code?: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  metaKey: boolean;
}

/** The action for a key press, or null when the key is not a shortcut. */
export function shortcutFor(e: KeyLike): ShortcutAction | null {
  if (e.altKey && !e.ctrlKey && !e.metaKey && !e.shiftKey) {
    const digit =
      /^Digit([1-9])$/.exec(e.code ?? "")?.[1] ?? (/^[1-9]$/.test(e.key) ? e.key : null);
    return digit ? { type: "vfoAt", index: Number(digit) - 1 } : null;
  }
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
    case "/":
      return { type: "palette" };
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
    case "f":
      return { type: "focusFrequency" };
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

/** The palette chord as shown on this platform (Cmd on Apple, Ctrl elsewhere). */
export const PALETTE_KEY: string =
  typeof navigator !== "undefined" && /Mac|iPhone|iPad/i.test(navigator.platform) ? "⌘K" : "Ctrl K";

export type ShortcutGroupTitle = "Tuning" | "Receiver" | "Spectrum" | "Layout";

export interface ShortcutDef {
  /** Stable id; command palette entries refer to it to show the key hint. */
  id: string;
  group: ShortcutGroupTitle;
  keys: readonly string[];
  label: string;
}

/**
 * The single list of keyboard shortcuts: the shortcuts dialog renders it grouped and the command
 * palette looks its key hints up by id. Keep in step with shortcutFor() and the digit readout.
 */
export const SHORTCUTS: readonly ShortcutDef[] = [
  { id: "palette", group: "Layout", keys: [PALETTE_KEY, "/"], label: "Command palette" },
  { id: "focusFrequency", group: "Tuning", keys: ["F"], label: "Type a frequency" },
  { id: "step", group: "Tuning", keys: ["←", "→"], label: "Step the active VFO down / up" },
  { id: "bigStep", group: "Tuning", keys: ["Shift", "←/→"], label: "Ten steps at once" },
  {
    id: "digit",
    group: "Tuning",
    keys: ["↑", "↓"],
    label: "On a focused digit of the readout: step it (also Wheel)",
  },
  { id: "vfo", group: "Tuning", keys: ["↑", "↓"], label: "Next / previous VFO" },
  { id: "vfoAt", group: "Tuning", keys: ["Alt", "1–9"], label: "Select VFO 1 to 9" },
  { id: "newVfo", group: "Tuning", keys: ["N"], label: "New VFO at the display centre" },
  {
    id: "delete",
    group: "Tuning",
    keys: ["Delete"],
    label: "Remove the active VFO (undo from the notice)",
  },
  { id: "bookmark", group: "Tuning", keys: ["B"], label: "Bookmark the active VFO" },
  { id: "mode123", group: "Receiver", keys: ["1", "2", "3"], label: "Mode WFM / NFM / AM" },
  {
    id: "mode456",
    group: "Receiver",
    keys: ["4", "5", "6"],
    label: "Mode USB / LSB / CW (bandwidth resets to the mode default)",
  },
  { id: "listen", group: "Receiver", keys: ["M"], label: "Mute / unmute the active VFO" },
  { id: "autoSquelch", group: "Receiver", keys: ["S"], label: "Squelch Auto on / off" },
  { id: "zoom", group: "Spectrum", keys: ["+", "−"], label: "Zoom in / out" },
  { id: "zoomReset", group: "Spectrum", keys: ["0"], label: "Reset zoom" },
  {
    id: "wheelZoom",
    group: "Spectrum",
    keys: ["Wheel"],
    label: "Zoom around the pointer (also over a VFO)",
  },
  {
    id: "wheelTune",
    group: "Spectrum",
    keys: ["Shift", "Wheel"],
    label: "Tune the active VFO by one step per notch",
  },
  { id: "inspector", group: "Layout", keys: ["]"], label: "Show / hide the inspector" },
  { id: "help", group: "Layout", keys: ["?"], label: "Show this list" },
];

/** Key caps of a registered shortcut, or null. */
export function shortcutKeys(id: string): readonly string[] | null {
  return SHORTCUTS.find((s) => s.id === id)?.keys ?? null;
}

const GROUP_ORDER: readonly ShortcutGroupTitle[] = ["Tuning", "Receiver", "Spectrum", "Layout"];

/** The registry grouped for display. */
export const SHORTCUT_GROUPS: readonly {
  title: ShortcutGroupTitle;
  items: readonly ShortcutDef[];
}[] = GROUP_ORDER.map((title) => ({
  title,
  items: SHORTCUTS.filter((s) => s.group === title),
}));
