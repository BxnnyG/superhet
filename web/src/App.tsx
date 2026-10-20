import { useCallback, useEffect, useState } from "react";
import { Sidebar } from "./shell/Sidebar";
import { StatusBar } from "./shell/StatusBar";
import { TopBar } from "./shell/TopBar";
import { useReceiver } from "./shell/useReceiver";
import { SpectrumView } from "./spectrum/SpectrumView";
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
        <main className="min-w-0 flex-1 p-3">
          <h1 className="sr-only">Superhet spectrum and waterfall</h1>
          <div className="size-full overflow-hidden rounded-xl border border-border-strong">
            <SpectrumView
              device={device}
              controller={r.levelController}
              tuning={r.tuning}
              vfoCtl={r.vfoCtl}
              step={r.step}
              createVfoAt={r.createVfoAt}
              metrics={r.metrics}
              registerView={r.registerView}
              onStatus={r.setStatus}
              onInfo={r.setInfo}
              onError={r.setSpectrumError}
            />
          </div>
        </main>
      </div>
      <StatusBar audio={r.audio} metrics={r.metrics} />
    </div>
  );
}
