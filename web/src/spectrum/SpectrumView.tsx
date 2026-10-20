import { useEffect, useRef, useState } from "react";
import type { SpectrumFrame } from "./frame";
import { SpectrumPlot } from "./plot";
import { SpectrumSocket, clampBins, type SocketStatus } from "./socket";
import { Waterfall } from "./waterfall";

interface Props {
  device?: string;
}

interface Info {
  centerHz: number;
  sampleRate: number;
}

const STATUS_LABEL: Record<SocketStatus, string> = {
  connecting: "Connecting…",
  connected: "Connected",
  reconnecting: "Reconnecting…",
};

const STATUS_COLOR: Record<SocketStatus, string> = {
  connecting: "bg-amber-400",
  connected: "bg-emerald-400",
  reconnecting: "bg-red-500",
};

/**
 * Live spectrum + waterfall. Per-frame work happens in the imperative renderers wired up in
 * the effect below; React state only changes on connection status and span changes.
 */
export function SpectrumView({ device = "dev0" }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<HTMLCanvasElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const [status, setStatus] = useState<SocketStatus>("connecting");
  const [info, setInfo] = useState<Info | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    const plotCanvas = plotRef.current;
    const wfCanvas = waterfallRef.current;
    if (!container || !plotCanvas || !wfCanvas) return;

    let plot: SpectrumPlot;
    let waterfall: Waterfall;
    try {
      plot = new SpectrumPlot(plotCanvas);
      waterfall = new Waterfall(wfCanvas);
    } catch (e) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- one-off setup failure
      setError(e instanceof Error ? e.message : String(e));
      return;
    }

    let last: Info | null = null;
    const socket = new SpectrumSocket({
      device,
      getBins: () => clampBins(container.clientWidth * window.devicePixelRatio),
      onFrame: (frame: SpectrumFrame) => {
        plot.push(frame);
        waterfall.push(frame);
        if (!last || last.centerHz !== frame.centerHz || last.sampleRate !== frame.sampleRate) {
          last = { centerHz: frame.centerHz, sampleRate: frame.sampleRate };
          setInfo(last);
        }
      },
      onStatus: setStatus,
      onError: (msg) => setError(msg),
    });

    const layout = () => {
      const dpr = window.devicePixelRatio;
      const w = container.clientWidth * dpr;
      plot.resize(w, plotCanvas.clientHeight * dpr, dpr);
      waterfall.resize(w, wfCanvas.clientHeight * dpr);
      socket.notifyResize();
    };
    layout();
    const observer = new ResizeObserver(layout);
    observer.observe(container);
    socket.start();

    return () => {
      observer.disconnect();
      socket.stop();
      plot.destroy();
      waterfall.destroy();
    };
  }, [device]);

  return (
    <section className="flex h-full min-h-0 flex-col gap-2">
      <header className="flex items-center gap-3 text-sm text-neutral-300">
        <span className="flex items-center gap-2" role="status">
          <span className={`inline-block size-2.5 rounded-full ${STATUS_COLOR[status]}`} />
          {STATUS_LABEL[status]}
        </span>
        <span className="text-neutral-500">{device}</span>
        {info && (
          <span className="font-mono tabular-nums">
            {(info.centerHz / 1e6).toFixed(3)} MHz · span {(info.sampleRate / 1e6).toFixed(3)} MHz
          </span>
        )}
        {error && (
          <span className="text-red-400" role="alert">
            {error}
          </span>
        )}
      </header>
      <div ref={containerRef} className="flex min-h-0 flex-1 flex-col overflow-hidden rounded">
        <canvas ref={plotRef} className="block h-56 w-full shrink-0" />
        <canvas ref={waterfallRef} className="block min-h-0 w-full flex-1" />
      </div>
    </section>
  );
}
