import { useEffect, useRef } from "react";
import { DEFAULT_LEVEL_SETTINGS, LevelController } from "../spectrum/levels";
import { AXIS_HEIGHT, SpectrumPlot } from "../spectrum/plot";
import { SpectrumSocket, clampBins } from "../spectrum/socket";
import { Waterfall } from "../spectrum/waterfall";
import { shareApi } from "./paths";

/**
 * Read-only spectrum and waterfall of a share link. The canvases are fed straight from the
 * socket (outside React renders); the socket is the token-scoped one and never probes a login
 * session.
 */
export function ShareSpectrum({
  token,
  compact = false,
  onStatus,
}: {
  token: string;
  compact?: boolean;
  onStatus?: (status: string) => void;
}) {
  const plotRef = useRef<HTMLCanvasElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const statusRef = useRef(onStatus);
  useEffect(() => {
    statusRef.current = onStatus;
  });

  useEffect(() => {
    const plotCanvas = plotRef.current;
    const wfCanvas = waterfallRef.current;
    if (!plotCanvas || !wfCanvas) return;
    const levels = new LevelController({ ...DEFAULT_LEVEL_SETTINGS });
    let plot: SpectrumPlot;
    try {
      plot = new SpectrumPlot(plotCanvas, levels.range);
    } catch {
      statusRef.current?.("unsupported");
      return;
    }
    let waterfall: Waterfall | null = null;
    try {
      waterfall = new Waterfall(wfCanvas, levels.range);
    } catch {
      waterfall = null;
    }
    const socket = new SpectrumSocket({
      device: "",
      url: shareApi(token).spectrum(),
      session: false,
      getBins: () => clampBins(wfCanvas.clientWidth * window.devicePixelRatio),
      onFrame: (frame) => {
        levels.onFrame(frame, performance.now());
        plot.push(frame);
        waterfall?.push(frame);
      },
      onStatus: (s) => statusRef.current?.(s),
      onError: (m) => statusRef.current?.(m),
    });
    const layout = () => {
      const dpr = window.devicePixelRatio;
      plot.resize(plotCanvas.clientWidth * dpr, plotCanvas.clientHeight * dpr, dpr);
      waterfall?.resize(wfCanvas.clientWidth * dpr, wfCanvas.clientHeight * dpr);
      socket.notifyResize();
    };
    layout();
    const observer = new ResizeObserver(layout);
    observer.observe(plotCanvas);
    observer.observe(wfCanvas);
    const offLevels = levels.onChange(() => {
      plot.redraw();
      waterfall?.redraw();
    });
    socket.start();
    return () => {
      offLevels();
      observer.disconnect();
      socket.stop();
      plot.destroy();
      waterfall?.destroy();
    };
  }, [token]);

  return (
    <div
      data-testid="share-spectrum"
      className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-border-strong"
    >
      <canvas
        ref={plotRef}
        aria-hidden="true"
        className={compact ? "h-16 w-full shrink-0" : "h-40 w-full shrink-0"}
        style={{ minHeight: AXIS_HEIGHT * 2 }}
      />
      <canvas ref={waterfallRef} aria-label="Waterfall" className="min-h-0 w-full flex-1" />
    </div>
  );
}
