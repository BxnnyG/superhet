import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { AudioControls } from "../audio/AudioControls";
import { AudioController } from "../audio/controller";
import { VfoController } from "../vfo/controller";
import { defaultModeFor, snapFrequency, stepFor, toMarker, type StepChoice } from "../vfo/model";
import { VfoPanel } from "../vfo/VfoPanel";
import type { SpectrumFrame } from "./frame";
import { LevelController, MIN_SPAN_DB, loadLevelSettings, saveLevelSettings } from "./levels";
import { SpectrumInteraction } from "./interaction";
import { PLOT_LEFT_PAD, SpectrumPlot } from "./plot";
import { SpectrumSocket, clampBins, type SocketStatus } from "./socket";
import { TuningControls } from "./TuningControls";
import { TuningController } from "./tuning";
import { ViewController, deviceSpan } from "./view";
import { Waterfall } from "./waterfall";

interface Props {
  device?: string;
}

interface Info {
  centerHz: number;
  sampleRate: number;
  minDb: number;
  maxDb: number;
}

interface LevelsUi {
  auto: boolean;
  lowDb: number;
  highDb: number;
}

/** Auto-level readout/slider refresh period; keeps React out of the per-frame path. */
const AUTO_UI_REFRESH_MS = 500;

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
  const overlayRef = useRef<HTMLCanvasElement>(null);
  const [status, setStatus] = useState<SocketStatus>("connecting");
  const [info, setInfo] = useState<Info | null>(null);
  const [error, setError] = useState<string | null>(null);
  const resetZoom = useRef<() => void>(() => undefined);
  const [controller] = useState(() => new LevelController(loadLevelSettings()));
  const tuning = useMemo(() => new TuningController({ device }), [device]);
  const tuningSnap = useSyncExternalStore(tuning.subscribe, tuning.getSnapshot);
  const vfoCtl = useMemo(() => new VfoController({ device }), [device]);
  const vfoSnap = useSyncExternalStore(vfoCtl.subscribe, vfoCtl.getSnapshot);
  const audio = useMemo(() => new AudioController({ device }), [device]);
  const audioSnap = useSyncExternalStore(audio.subscribe, audio.getSnapshot);
  const [step, setStep] = useState<StepChoice>("auto");
  // Pointer callbacks live in a long-running effect; they read the latest values from here.
  const live = useRef({ step });
  useEffect(() => {
    live.current = { step };
  });
  const createVfoAt = useCallback(
    (hz: number | null) => {
      if (hz === null) return;
      const mode = defaultModeFor(hz);
      vfoCtl.create({ freqHz: snapFrequency(hz, stepFor(live.current.step, mode)), mode });
    },
    [vfoCtl],
  );
  const [levels, setLevels] = useState<LevelsUi>(() => ({
    auto: controller.auto,
    lowDb: Math.round(controller.range.lowDb),
    highDb: Math.round(controller.range.highDb),
  }));

  useEffect(() => {
    const container = containerRef.current;
    const plotCanvas = plotRef.current;
    const wfCanvas = waterfallRef.current;
    const overlay = overlayRef.current;
    if (!container || !plotCanvas || !wfCanvas || !overlay) return;

    let plot: SpectrumPlot;
    let waterfall: Waterfall;
    try {
      plot = new SpectrumPlot(plotCanvas, controller.range);
      waterfall = new Waterfall(wfCanvas, controller.range);
    } catch (e) {
      // eslint-disable-next-line react-hooks/set-state-in-effect -- one-off setup failure
      setError(e instanceof Error ? e.message : String(e));
      return;
    }

    let last: Info | null = null;
    let interaction: SpectrumInteraction | null = null;
    const socket = new SpectrumSocket({
      device,
      getBins: () => clampBins(wfCanvas.clientWidth * window.devicePixelRatio),
      onFrame: (frame: SpectrumFrame) => {
        controller.onFrame(frame, performance.now());
        plot.push(frame);
        waterfall.push(frame);
        interaction?.setFrameSpan(frame.centerHz, frame.sampleRate);
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
          setInfo(last);
        }
      },
      onStatus: (s) => {
        setStatus(s);
        if (s === "connected") void tuning.refresh();
      },
      onError: (msg) => setError(msg),
    });

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
      onVfoCreate: (hz) => createVfoAt(hz),
    });
    const pushVfos = () => {
      const snap = vfoCtl.getSnapshot();
      interaction?.setVfos(snap.vfos.map(toMarker), snap.selectedId);
    };
    pushVfos();
    const offVfos = vfoCtl.subscribe(pushVfos);
    // Keep the zoom limits in step with the confirmed device tuning.
    let lastState: unknown = null;
    const offTuning = tuning.subscribe(() => {
      const state = tuning.getSnapshot().state;
      if (!state || state === lastState) return;
      lastState = state;
      view.setDevice(deviceSpan(state.centerHz, state.sampleRate));
    });
    resetZoom.current = () => view.reset();
    tuning.start();

    const layout = () => {
      const dpr = window.devicePixelRatio;
      const w = container.clientWidth * dpr;
      plot.resize(w, plotCanvas.clientHeight * dpr, dpr);
      waterfall.resize(wfCanvas.clientWidth * dpr, wfCanvas.clientHeight * dpr);
      interaction?.resize(w, overlay.clientHeight * dpr, dpr);
      socket.notifyResize();
    };
    layout();
    const observer = new ResizeObserver(layout);
    observer.observe(container);
    socket.start();

    // Manual changes redraw immediately; auto mode redraws with the next frame anyway.
    const offChange = controller.onChange(() => {
      plot.redraw();
      waterfall.redraw();
    });
    // Mirror the auto-estimated range into the controls at a low rate.
    const timer = window.setInterval(() => {
      if (!controller.auto) return;
      const lowDb = Math.round(controller.range.lowDb);
      const highDb = Math.round(controller.range.highDb);
      setLevels((prev) =>
        prev.auto && prev.lowDb === lowDb && prev.highDb === highDb
          ? prev
          : { auto: true, lowDb, highDb },
      );
    }, AUTO_UI_REFRESH_MS);

    return () => {
      resetZoom.current = () => undefined;
      offTuning();
      offVfos();
      tuning.stop();
      view.dispose();
      interaction?.destroy();
      window.clearInterval(timer);
      offChange();
      observer.disconnect();
      socket.stop();
      plot.destroy();
      waterfall.destroy();
    };
  }, [device, controller, tuning, vfoCtl, createVfoAt]);

  // VFO list polling and the audio socket live as long as the view.
  useEffect(() => {
    vfoCtl.start();
    audio.start();
    return () => {
      vfoCtl.stop();
      audio.stop();
    };
  }, [vfoCtl, audio]);

  // Subscribe to the audio of every VFO (frames also drive the S-meter while audio is off).
  const vfoIds = vfoSnap.vfos.map((v) => v.id).join(",");
  useEffect(() => {
    audio.setVfos(vfoIds === "" ? [] : vfoIds.split(",").map(Number));
  }, [audio, vfoIds]);

  // Escape deselects the VFO again (a plain click then tunes the device centre).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target;
      const typing = t instanceof HTMLElement && /^(INPUT|SELECT|TEXTAREA)$/.test(t.tagName);
      if (e.key === "Escape" && !typing) vfoCtl.select(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [vfoCtl]);

  const applyManual = (lowDb: number, highDb: number) => {
    controller.setManual(lowDb, highDb);
    const next = { auto: false, lowDb: controller.range.lowDb, highDb: controller.range.highDb };
    setLevels(next);
    saveLevelSettings(controller.settings());
  };

  const applyAuto = () => {
    controller.setAuto();
    setLevels((prev) => ({ ...prev, auto: true }));
    saveLevelSettings(controller.settings());
  };

  const dbMin = info?.minDb ?? -120;
  const dbMax = info?.maxDb ?? 0;

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
            view {(info.centerHz / 1e6).toFixed(3)} MHz ± {(info.sampleRate / 2e3).toFixed(1)} kHz
          </span>
        )}
        <button
          type="button"
          className="rounded border border-neutral-600 px-2 py-0.5"
          title="Show the full device span (or double-click the display)"
          onClick={() => resetZoom.current()}
        >
          Full span
        </button>
        <div className="ml-auto flex items-center gap-3" role="group" aria-label="Display levels">
          <label className="flex items-center gap-1">
            <input
              type="checkbox"
              checked={levels.auto}
              onChange={(e) => {
                if (e.target.checked) applyAuto();
                else applyManual(controller.range.lowDb, controller.range.highDb);
              }}
            />
            Auto
          </label>
          <label className="flex items-center gap-1">
            Floor
            <input
              type="range"
              className="w-24"
              min={dbMin}
              max={dbMax - MIN_SPAN_DB}
              step={1}
              value={levels.lowDb}
              onChange={(e) => {
                const low = Number(e.target.value);
                applyManual(low, Math.max(levels.highDb, low + MIN_SPAN_DB));
              }}
            />
            <output className="w-14 text-right font-mono tabular-nums">{levels.lowDb} dB</output>
          </label>
          <label className="flex items-center gap-1">
            Ceiling
            <input
              type="range"
              className="w-24"
              min={dbMin + MIN_SPAN_DB}
              max={dbMax}
              step={1}
              value={levels.highDb}
              onChange={(e) => {
                const high = Number(e.target.value);
                applyManual(Math.min(levels.lowDb, high - MIN_SPAN_DB), high);
              }}
            />
            <output className="w-14 text-right font-mono tabular-nums">{levels.highDb} dB</output>
          </label>
          <button
            type="button"
            className="rounded border border-neutral-600 px-2 py-0.5 disabled:opacity-40"
            disabled={levels.auto}
            onClick={applyAuto}
          >
            Reset to auto
          </button>
        </div>
        {error && (
          <span className="text-red-400" role="alert">
            {error}
          </span>
        )}
      </header>
      <TuningControls tuning={tuningSnap} onTune={(patch) => tuning.request(patch)} />
      <AudioControls audio={audio} snapshot={audioSnap} />
      <VfoPanel
        vfos={vfoSnap}
        audio={audio}
        audioSnap={audioSnap}
        step={step}
        onStep={setStep}
        onCreate={() => createVfoAt(tuningSnap.display?.centerHz ?? null)}
        onSelect={(id) => vfoCtl.select(id)}
        onUpdate={(id, patch) => vfoCtl.update(id, patch)}
        onRemove={(id) => vfoCtl.remove(id)}
      />
      <div
        ref={containerRef}
        className="relative flex min-h-0 flex-1 flex-col overflow-hidden rounded"
      >
        <canvas ref={plotRef} className="block h-56 w-full shrink-0" />
        <canvas
          ref={waterfallRef}
          className="block min-h-0 flex-1"
          style={{ marginLeft: PLOT_LEFT_PAD }}
        />
        <canvas
          ref={overlayRef}
          className="absolute inset-0 size-full cursor-crosshair touch-none"
          aria-label="Spectrum: wheel zooms, shift-drag pans, click moves the selected VFO or tunes the centre, Alt/Ctrl-click creates a VFO, drag a VFO to retune it, double-click resets zoom"
        />
      </div>
    </section>
  );
}
