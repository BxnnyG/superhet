import { useCallback, useEffect, useState } from "react";
import { BookmarkEditorHost } from "./bookmarks/BookmarkEditor";
import { Sidebar } from "./shell/Sidebar";
import { StatusBar } from "./shell/StatusBar";
import { TopBar } from "./shell/TopBar";
import { useReceiver } from "./shell/useReceiver";
import { SpectrumView } from "./spectrum/SpectrumView";
import { ReceiverDock } from "./vfo/ReceiverDock";
import { Toaster } from "./ui";
import { useTheme } from "./ui/theme";

interface Props {
  device?: string;
}

export function App({ device = "dev0" }: Props) {
  const r = useReceiver(device);
  const [theme, setTheme] = useTheme();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const closeDrawer = useCallback(() => setDrawerOpen(false), []);

  useEffect(() => {
    if (!drawerOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setDrawerOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [drawerOpen]);

  return (
    <div className="flex h-full flex-col bg-surface-0 text-fg">
      <TopBar
        r={r}
        theme={theme}
        onTheme={setTheme}
        drawerOpen={drawerOpen}
        onMenu={() => setDrawerOpen((o) => !o)}
      />
      {r.errors.length > 0 && (
        <div
          role="alert"
          className="shrink-0 border-b border-border bg-danger-soft px-4 py-1.5 text-xs text-danger"
        >
          {r.errors.join(" · ")}
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <Sidebar r={r} open={drawerOpen} onClose={closeDrawer} />
        <main className="flex min-h-0 min-w-0 flex-1 flex-col gap-3 overflow-y-auto p-3">
          <h1 className="sr-only">Superhet spectrum and waterfall</h1>
          <div className="min-h-48 flex-1 shrink-0 overflow-hidden rounded-xl border border-border-strong">
            <SpectrumView
              device={device}
              controller={r.levelController}
              tuning={r.spectrumTuning}
              vfoCtl={r.vfoCtl}
              step={r.step}
              createVfoAt={r.createVfoAt}
              overlaySettings={r.overlay}
              bookmarks={r.bookmarks}
              onAnnotationTune={r.tuneTo}
              metrics={r.metrics}
              registerView={r.registerView}
              onStatus={r.setStatus}
              onInfo={r.setInfo}
              onError={r.setSpectrumError}
            />
          </div>
          <ReceiverDock
            vfos={r.vfoSnap}
            audio={r.audio}
            audioSnap={r.audioSnap}
            locked={r.locks.length > 0}
            onCreate={() => r.createVfoAt(r.tuningSnap.display?.centerHz ?? null)}
            onSelect={(id) => r.vfoCtl.select(id)}
            onUpdate={(id, patch) => r.vfoCtl.update(id, patch)}
            onRemove={(id) => r.vfoCtl.remove(id)}
          />
        </main>
      </div>
      <StatusBar audio={r.audio} metrics={r.metrics} />
      <BookmarkEditorHost store={r.bookmarks} />
      <Toaster />
    </div>
  );
}
