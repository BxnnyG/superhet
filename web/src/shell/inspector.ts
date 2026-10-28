// State of the right-hand inspector: which tab is showing and whether it is open. On wide screens
// the open state is a persisted preference; on narrower ones the inspector is a transient sheet.

import { useCallback, useEffect, useState } from "react";
import { SHELL_QUERY, useMediaQuery } from "./useMediaQuery";

export const INSPECTOR_TABS = [
  "receiver",
  "bookmarks",
  "channels",
  "decoders",
  "dab",
  "display",
  "source",
] as const;
export type InspectorTab = (typeof INSPECTOR_TABS)[number];

/** Tabs the viewer may see: guests have no bookmarks, and decoder history only if the policy shows it. */
export function visibleTabs(perms: {
  canReadBookmarks: boolean;
  canSeeDecoders: boolean;
}): readonly InspectorTab[] {
  return INSPECTOR_TABS.filter(
    (t) =>
      (t !== "bookmarks" || perms.canReadBookmarks) &&
      ((t !== "decoders" && t !== "dab") || perms.canSeeDecoders),
  );
}

export const INSPECTOR_LABEL: Record<InspectorTab, string> = {
  receiver: "Receiver",
  bookmarks: "Bookmarks",
  channels: "Channels",
  decoders: "Decoders",
  dab: "DAB",
  display: "Display",
  source: "Source",
};

export const INSPECTOR_STORAGE_KEY = "superhet.inspector.open.v1";

export function loadInspectorOpen(): boolean {
  try {
    return window.localStorage.getItem(INSPECTOR_STORAGE_KEY) !== "0";
  } catch {
    return true;
  }
}

export function saveInspectorOpen(open: boolean): void {
  try {
    if (open) window.localStorage.removeItem(INSPECTOR_STORAGE_KEY);
    else window.localStorage.setItem(INSPECTOR_STORAGE_KEY, "0");
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

export interface InspectorState {
  tab: InspectorTab;
  /** Whether the inspector is visible right now (persisted panel or transient sheet). */
  open: boolean;
  /** Wide layout: the inspector is a docked column rather than a sheet. */
  docked: boolean;
  setTab: (tab: InspectorTab) => void;
  /** Open the inspector on a tab. */
  show: (tab: InspectorTab) => void;
  /** The `]` shortcut and the toggle buttons. */
  toggle: () => void;
  close: () => void;
}

export function useInspector(): InspectorState {
  const docked = useMediaQuery(SHELL_QUERY);
  const [tab, setTab] = useState<InspectorTab>("receiver");
  const [dockedOpen, setDockedOpen] = useState(loadInspectorOpen);
  const [sheetOpen, setSheetOpen] = useState(false);
  const open = docked ? dockedOpen : sheetOpen;

  const setOpen = useCallback(
    (next: boolean) => {
      if (docked) {
        setDockedOpen(next);
        saveInspectorOpen(next);
      } else {
        setSheetOpen(next);
      }
    },
    [docked],
  );
  const show = useCallback(
    (next: InspectorTab) => {
      setTab(next);
      setOpen(true);
    },
    [setOpen],
  );
  const toggle = useCallback(() => setOpen(!open), [open, setOpen]);
  const close = useCallback(() => setOpen(false), [setOpen]);

  // A sheet closes with Escape; the docked column does not (Escape belongs to the controls in it).
  useEffect(() => {
    if (docked || !sheetOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSheetOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [docked, sheetOpen]);

  return { tab, open, docked, setTab, show, toggle, close };
}
