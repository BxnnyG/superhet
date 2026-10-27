import { useMemo } from "react";
import { usePermissions } from "../auth/context";
import { downloadEventLog } from "../decoders/api";
import { NO_FILTER } from "../decoders/events";
import { visibleTabs, type InspectorState } from "../shell/inspector";
import { EDIT_FREQUENCY_EVENT, OPEN_SETTINGS_EVENT, emitAppEvent } from "../shell/appEvents";
import type { Receiver } from "../shell/useReceiver";
import { runShortcut, type ShortcutContext } from "../shell/useShortcuts";
import { Palette } from "./Palette";
import type { PaletteDeps } from "./items";
import { loadRecent, pushRecent, saveRecent } from "./recent";

interface Props {
  r: Receiver;
  inspector: InspectorState;
  shortcutCtx: ShortcutContext;
  onClose: () => void;
}

/** Wires the palette to the receiver; mounted only while the palette is open. */
export function PaletteHost({ r, inspector, shortcutCtx, onClose }: Props) {
  const recent = useMemo(() => loadRecent(), []);
  const perms = usePermissions();
  const deps: PaletteDeps = {
    tabs: visibleTabs(perms),
    canBookmark: perms.canEditBookmarks,
    vfos: r.vfoSnap.vfos,
    selectedId: r.vfoSnap.selectedId,
    bookmarks: r.bookmarkSnap.bookmarks,
    recent,
    channels: {
      plans: r.channelSnap.plans,
      region: r.channelSnap.region,
      tuneChannel: r.tuneChannel,
    },
    audioEnabled: r.audioSnap.enabled,
    tune: (target, newVfo) => (newVfo ? r.tuneNew(target) : r.tuneTo(target)),
    remember: (entry) => saveRecent(pushRecent(loadRecent(), entry)),
    act: (action) => {
      if (action.type === "focusFrequency") emitAppEvent(EDIT_FREQUENCY_EVENT);
      else runShortcut(action, shortcutCtx);
    },
    selectVfo: (id) => r.vfoCtl.select(id),
    showTab: inspector.show,
    toggleAudio: () => void (r.audioSnap.enabled ? r.audio.disable() : r.audio.enable()),
    exportLog: () => downloadEventLog(r.device, NO_FILTER),
    openSettings: () => emitAppEvent(OPEN_SETTINGS_EVENT),
  };
  return <Palette deps={deps} onClose={onClose} />;
}
