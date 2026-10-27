import { useCallback, useState } from "react";
import { BookmarkEditorHost } from "./bookmarks/BookmarkEditor";
import { PaletteHost } from "./palette/PaletteHost";
import { EDIT_FREQUENCY_EVENT, emitAppEvent } from "./shell/appEvents";
import { Inspector } from "./shell/Inspector";
import { useInspector } from "./shell/inspector";
import { MobileTabBar } from "./shell/MobileTabBar";
import { ShortcutsDialog } from "./shell/ShortcutsDialog";
import { StatusBar } from "./shell/StatusBar";
import { TopBar } from "./shell/TopBar";
import { TuningStrip } from "./shell/TuningStrip";
import { useReceiver } from "./shell/useReceiver";
import { useShortcuts, type ShortcutContext } from "./shell/useShortcuts";
import { VfoTabs } from "./shell/VfoTabs";
import { SpectrumView } from "./spectrum/SpectrumView";
import { Toaster } from "./ui";
import { useTheme } from "./ui/theme";

interface Props {
  device?: string;
}

/**
 * Fixed shell, the page never scrolls: top bar, the tuning strip, the VFO tab row, the spectrum
 * with the waterfall filling what is left, the inspector on the right and the status bar. Only the
 * inspector tab bodies scroll.
 */
export function App({ device = "dev0" }: Props) {
  const r = useReceiver(device);
  const [theme, setTheme] = useTheme();
  const inspector = useInspector();
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const openShortcuts = useCallback(() => setShortcutsOpen(true), []);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const openPalette = useCallback(() => setPaletteOpen(true), []);
  const closePalette = useCallback(() => setPaletteOpen(false), []);
  const togglePalette = useCallback(() => setPaletteOpen((o) => !o), []);

  const shortcutCtx: ShortcutContext = {
    vfoCtl: r.vfoCtl,
    audio: r.audio,
    autoSquelch: r.autoSquelch,
    step: r.step,
    zoom: (factor) => r.viewActions.current.zoom(factor),
    resetZoom: () => r.viewActions.current.reset(),
    createVfoAt: r.createVfoAt,
    centerHz: r.tuningSnap.display?.centerHz ?? null,
    startBookmark: r.startBookmark,
    openHelp: openShortcuts,
    toggleInspector: inspector.toggle,
    focusFrequency: () => emitAppEvent(EDIT_FREQUENCY_EVENT),
    openPalette,
    togglePalette,
    stepChannels: r.stepChannels,
  };
  useShortcuts(shortcutCtx);

  return (
    <div className="flex h-full flex-col overflow-hidden bg-surface-0 text-fg">
      <TopBar
        r={r}
        theme={theme}
        onTheme={setTheme}
        onShortcuts={openShortcuts}
        onPalette={openPalette}
        inspector={inspector}
      />
      {r.errors.length > 0 && (
        <div
          role="alert"
          className="shrink-0 border-b border-border bg-danger-soft px-4 py-1.5 text-xs text-danger"
        >
          {r.errors.join(" · ")}
        </div>
      )}
      <TuningStrip r={r} />
      <div className="flex min-h-0 flex-1">
        <main className="flex min-h-0 min-w-0 flex-1 flex-col">
          <h1 className="sr-only">Superhet spectrum and waterfall</h1>
          <VfoTabs r={r} />
          <div className="min-h-0 flex-1 p-3">
            <div className="size-full overflow-hidden rounded-xl border border-border-strong">
              <SpectrumView
                device={device}
                controller={r.levelController}
                tuning={r.spectrumTuning}
                vfoCtl={r.vfoCtl}
                step={r.step}
                createVfoAt={r.createVfoAt}
                overlaySettings={r.overlay}
                bookmarks={r.bookmarks}
                channels={r.channels}
                onAnnotationTune={(t, newVfo) => (newVfo ? r.tuneNew(t) : r.tuneTo(t))}
                metrics={r.metrics}
                registerView={r.registerView}
                onStatus={r.setStatus}
                onInfo={r.setInfo}
                onError={r.setSpectrumError}
              />
            </div>
          </div>
        </main>
        <Inspector r={r} inspector={inspector} />
      </div>
      <MobileTabBar inspector={inspector} />
      <StatusBar audio={r.audio} metrics={r.metrics} />
      <BookmarkEditorHost store={r.bookmarks} />
      {paletteOpen && (
        <PaletteHost r={r} inspector={inspector} shortcutCtx={shortcutCtx} onClose={closePalette} />
      )}
      {shortcutsOpen && <ShortcutsDialog onClose={() => setShortcutsOpen(false)} />}
      <Toaster />
    </div>
  );
}
