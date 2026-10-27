import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type PointerEvent,
} from "react";
import { enabledCategories, type OverlayStore } from "../bandplan/settings";
import { builtinBandPlan, buildBandPlan } from "../bandplan/model";
import type { BookmarkStore } from "../bookmarks/store";
import { overlayPlans } from "../channels/overlay";
import type { ChannelStore } from "../channels/store";
import type { SpectrumInfo, ViewActions } from "../shell/useReceiver";
import type { SpectrumMetrics } from "../shell/metrics";
import type { VfoController } from "../vfo/controller";
import { stepFor, stepFrequency, snapFrequency, toMarker, type StepChoice } from "../vfo/model";
import { AnnotationLayer, type AnnotationTarget } from "./annotations";
import type { SpectrumFrame } from "./frame";
import type { LevelController } from "./levels";
import { SpectrumInteraction } from "./interaction";
import { CANVAS_BG } from "./palette";
import { AXIS_HEIGHT, PLOT_LEFT_PAD, SpectrumPlot } from "./plot";
import { snapClick } from "./preview";
import { getWheelMode } from "./wheelMode";
import { SpectrumSocket, clampBins, type SocketStatus } from "./socket";
import type { TuningController } from "./tuning";
import { ViewController, deviceSpan } from "./view";
import { Waterfall } from "./waterfall";

interface Props {
  device: string;
  controller: LevelController;
  tuning: TuningController;
  vfoCtl: VfoController;
  step: StepChoice;
  createVfoAt: (hz: number | null) => void;
  /** Band plan / bookmark overlay settings and the bookmarks drawn on the spectrum. */
  overlaySettings: OverlayStore;
  bookmarks: BookmarkStore;
  /** Custom channel plans drawn as ticks in the overlay (optional). */
  channels?: ChannelStore;
  /** A tunable channel or bookmark in the overlay strip was clicked (`newVfo`: Alt/Ctrl-click). */
  onAnnotationTune: (target: AnnotationTarget, newVfo: boolean) => void;
  metrics: SpectrumMetrics;
  registerView: (actions: ViewActions) => void;
  onStatus: (status: SocketStatus) => void;
  onInfo: (info: SpectrumInfo) => void;
  onError: (message: string) => void;
}

const SPLIT_KEY = "superhet.plotHeight";
const DEFAULT_PLOT_PX = 224;
const MIN_PANE_PX = 96;
const SPLITTER_PX = 6;
const KEY_STEP_PX = 16;

function loadPlotHeight(): number {
  try {
    const n = Number(window.localStorage.getItem(SPLIT_KEY));
    return Number.isFinite(n) && n >= MIN_PANE_PX ? n : DEFAULT_PLOT_PX;
  } catch {
    return DEFAULT_PLOT_PX;
  }
}

function savePlotHeight(px: number): void {
  try {
    window.localStorage.setItem(SPLIT_KEY, String(Math.round(px)));
  } catch {
    // Not persisted.
  }
}

/**
 * Live spectrum + waterfall with a draggable split. Per-frame work happens in the imperative
 * renderers wired up in the effect below; React state only changes on the split position.
 */
export function SpectrumView({
  device,
  controller,
  tuning,
  vfoCtl,
  step,
  createVfoAt,
  overlaySettings,
  bookmarks,
  channels,
  onAnnotationTune,
  metrics,
  registerView,
  onStatus,
  onInfo,
  onError,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<HTMLCanvasElement>(null);
  const waterfallRef = useRef<HTMLCanvasElement>(null);
  const overlayRef = useRef<HTMLCanvasElement>(null);
  const waterfallNoticeRef = useRef<HTMLDivElement>(null);
  const [plotHeight, setPlotHeight] = useState(loadPlotHeight);
  // Callbacks and values read by long-running listeners come from here, not from effect deps.
  const live = useRef({ step, registerView, onStatus, onInfo, onError, onAnnotationTune });
  useEffect(() => {
    live.current = { step, registerView, onStatus, onInfo, onError, onAnnotationTune };
  });

  useEffect(() => {
    const container = containerRef.current;
    const plotCanvas = plotRef.current;
    const wfCanvas = waterfallRef.current;
    const overlay = overlayRef.current;
    const wfNotice = waterfallNoticeRef.current;
    if (!container || !plotCanvas || !wfCanvas || !overlay) return;

    let plot: SpectrumPlot;
    try {
      plot = new SpectrumPlot(plotCanvas, controller.range);
    } catch (e) {
      live.current.onError(e instanceof Error ? e.message : String(e));
      return;
    }
    // The waterfall is optional: without WebGL2 everything else (state, tuning, VFOs, audio, the
    // 2D plot) keeps working and only the waterfall pane shows a notice.
    let waterfall: Waterfall | null = null;
    try {
      waterfall = new Waterfall(wfCanvas, controller.range);
    } catch {
      waterfall = null;
    }
    if (wfNotice) wfNotice.hidden = waterfall !== null;

    let last: SpectrumInfo | null = null;
    let interaction: SpectrumInteraction | null = null;
    const socket = new SpectrumSocket({
      device,
      getBins: () => clampBins(wfCanvas.clientWidth * window.devicePixelRatio),
      onFrame: (frame: SpectrumFrame) => {
        metrics.frames++;
        controller.onFrame(frame, performance.now());
        plot.push(frame);
        waterfall?.push(frame);
        interaction?.setFrameSpan(frame.centerHz, frame.sampleRate);
        view.setShown({
          startHz: frame.centerHz - frame.sampleRate / 2,
          endHz: frame.centerHz + frame.sampleRate / 2,
        });
        if (
          !last ||
          last.centerHz !== frame.centerHz ||
          last.sampleRate !== frame.sampleRate ||
          last.minDb !== frame.minDb ||
          last.maxDb !== frame.maxDb
        ) {
          last = {
            centerHz: frame.centerHz,
            sampleRate: frame.sampleRate,
            minDb: frame.minDb,
            maxDb: frame.maxDb,
          };
          live.current.onInfo(last);
        }
      },
      onStatus: (s) => {
        live.current.onStatus(s);
        if (s === "connected") void tuning.refresh();
      },
      onError: (msg) => live.current.onError(msg),
    });

    const annotations = new AnnotationLayer();
    const view = new ViewController({
      apply: (span) => socket.setView(span ? { startHz: span.startHz, endHz: span.endHz } : {}),
    });
    interaction = new SpectrumInteraction({
      canvas: overlay,
      leftPad: PLOT_LEFT_PAD,
      view,
      onTune: (hz) => tuning.request({ centerHz: hz }),
      onVfoSelect: (id) => vfoCtl.select(id),
      onVfoMove: (id, hz) => {
        const v = vfoCtl.getSnapshot().vfos.find((x) => x.id === id);
        if (v) vfoCtl.update(id, { freqHz: snapFrequency(hz, stepFor(live.current.step, v.mode)) });
      },
      // The same rule the click applies, so the hover preview shows the real landing frequency.
      snap: (hz, id) => {
        const v = id === null ? undefined : vfoCtl.getSnapshot().vfos.find((x) => x.id === id);
        return snapClick(hz, v ? stepFor(live.current.step, v.mode) : null);
      },
      onVfoCreate: (hz) => createVfoAt(hz),
      annotations,
      onAnnotationTune: (target, newVfo) => live.current.onAnnotationTune(target, newVfo),
      getWheelMode,
      onVfoStep: (id, steps) => {
        const v = vfoCtl.getSnapshot().vfos.find((x) => x.id === id);
        if (v) {
          const stepHz = stepFor(live.current.step, v.mode);
          vfoCtl.update(id, { freqHz: stepFrequency(v.freqHz, stepHz, steps) });
        }
      },
      onBandwidth: (id, bandwidthHz) => vfoCtl.update(id, { bandwidthHz }),
      // The axis strip sits at the bottom of the spectrum canvas.
      axis: () => {
        const bottom = plotCanvas.offsetTop + plotCanvas.clientHeight;
        return { top: bottom - AXIS_HEIGHT, bottom };
      },
    });
    const syncAnnotations = () => {
      const settings = overlaySettings.getSnapshot();
      annotations.setEnabled(enabledCategories(settings));
      annotations.setBookmarks(bookmarks.getSnapshot().bookmarks, settings.bookmarks);
      interaction?.refresh();
    };
    syncAnnotations();
    const syncPlan = () => {
      if (!channels) return;
      const custom = overlayPlans(channels.getSnapshot().custom);
      annotations.setPlan(
        custom.length > 0 ? buildBandPlan({ ranges: [], plans: custom }) : builtinBandPlan(),
      );
      interaction?.refresh();
    };
    syncPlan();
    const offChannels = channels?.subscribe(syncPlan);
    const offOverlay = overlaySettings.subscribe(syncAnnotations);
    const offBookmarks = bookmarks.subscribe(syncAnnotations);
    const pushVfos = () => {
      const snap = vfoCtl.getSnapshot();
      interaction?.setVfos(snap.vfos.map(toMarker), snap.selectedId);
    };
    pushVfos();
    const offVfos = vfoCtl.subscribe(pushVfos);
    // Keep the zoom limits in step with the confirmed device tuning.
    let lastState: unknown = null;
    const onTuning = () => {
      const state = tuning.getSnapshot().state;
      if (!state || state === lastState) return;
      lastState = state;
      // Until the first frame arrives (or if none ever does) the confirmed device span stands in
      // for the frame span so VFOs and the usable window are drawn right away.
      if (interaction && !interaction.hasFrameSpan()) {
        interaction.setFrameSpan(state.centerHz, state.sampleRate);
      }
      view.setDevice(deviceSpan(state.centerHz, state.sampleRate));
      interaction?.setWindow(state.window);
      plot.setWindow(state.window);
    };
    onTuning();
    const offTuning = tuning.subscribe(onTuning);
    live.current.registerView({
      reset: () => view.reset(),
      zoom: (factor) => view.zoom(0.5, factor),
    });
    tuning.start();

    const layout = () => {
      const dpr = window.devicePixelRatio;
      const w = container.clientWidth * dpr;
      plot.resize(w, plotCanvas.clientHeight * dpr, dpr);
      waterfall?.resize(wfCanvas.clientWidth * dpr, wfCanvas.clientHeight * dpr);
      interaction?.resize(w, overlay.clientHeight * dpr, dpr);
      socket.notifyResize();
    };
    layout();
    const observer = new ResizeObserver(layout);
    observer.observe(container);
    observer.observe(plotCanvas);
    socket.start();

    // Manual changes redraw immediately; auto mode redraws with the next frame anyway.
    const offChange = controller.onChange(() => {
      plot.redraw();
      waterfall?.redraw();
    });
    return () => {
      live.current.registerView({ reset: () => undefined, zoom: () => undefined });
      offTuning();
      offVfos();
      offOverlay();
      offBookmarks();
      offChannels?.();
      tuning.stop();
      view.dispose();
      interaction?.destroy();
      offChange();
      observer.disconnect();
      socket.stop();
      plot.destroy();
      waterfall?.destroy();
    };
  }, [
    device,
    controller,
    tuning,
    vfoCtl,
    createVfoAt,
    overlaySettings,
    bookmarks,
    channels,
    metrics,
  ]);

  const clampPlot = useCallback((px: number) => {
    const h = containerRef.current?.clientHeight ?? 0;
    const max = Math.max(MIN_PANE_PX, h - MIN_PANE_PX - SPLITTER_PX);
    return Math.round(Math.min(max, Math.max(MIN_PANE_PX, px)));
  }, []);

  const dragSplit = (e: PointerEvent<HTMLDivElement>) => {
    if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
    const rect = containerRef.current?.getBoundingClientRect();
    if (rect) setPlotHeight(clampPlot(e.clientY - rect.top - SPLITTER_PX / 2));
  };

  const splitKey = (e: KeyboardEvent<HTMLDivElement>) => {
    const delta = e.key === "ArrowUp" ? -KEY_STEP_PX : e.key === "ArrowDown" ? KEY_STEP_PX : 0;
    if (e.key === "Home") {
      e.preventDefault();
      setPlotHeight(DEFAULT_PLOT_PX);
      savePlotHeight(DEFAULT_PLOT_PX);
    } else if (delta !== 0) {
      e.preventDefault();
      const next = clampPlot(plotHeight + delta);
      setPlotHeight(next);
      savePlotHeight(next);
    }
  };

  return (
    <div
      ref={containerRef}
      className="relative flex size-full min-h-0 flex-col overflow-hidden"
      style={{ background: CANVAS_BG }}
    >
      <canvas
        ref={plotRef}
        className="block w-full min-h-24 shrink"
        style={{ flex: `0 1 ${plotHeight}px` }}
      />
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize spectrum and waterfall"
        aria-valuenow={plotHeight}
        aria-valuemin={MIN_PANE_PX}
        tabIndex={0}
        onPointerDown={(e) => e.currentTarget.setPointerCapture(e.pointerId)}
        onPointerMove={dragSplit}
        onPointerUp={() => savePlotHeight(plotHeight)}
        onKeyDown={splitKey}
        className="group/split focus-ring relative z-10 flex shrink-0 cursor-row-resize touch-none items-center justify-center"
        style={{ height: SPLITTER_PX }}
      >
        <span className="h-px w-full bg-zinc-800 transition-colors duration-150 group-hover/split:bg-indigo-400 group-focus-visible/split:bg-indigo-400" />
      </div>
      <div className="relative min-h-24 flex-1" style={{ marginLeft: PLOT_LEFT_PAD }}>
        <canvas ref={waterfallRef} className="block size-full" />
        <div
          ref={waterfallNoticeRef}
          hidden
          role="status"
          className="absolute inset-0 flex items-center justify-center px-4 text-center text-sm text-zinc-500"
        >
          Waterfall needs WebGL2, which this browser has disabled.
        </div>
      </div>
      <canvas
        ref={overlayRef}
        className="absolute inset-0 size-full cursor-crosshair touch-none"
        aria-label="Spectrum: wheel zooms (Shift+wheel tunes the selected VFO), drag pans, click moves the selected VFO or tunes the centre, Alt/Ctrl-click creates a VFO, drag a VFO to retune it, drag a passband edge to change its bandwidth, double-click resets zoom"
      />
    </div>
  );
}
