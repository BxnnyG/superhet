// Owns the receiver controllers (tuning, VFOs, audio, display levels) and the coarse UI state
// derived from them. Per-frame work stays inside the controllers and canvas renderers.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { AudioController } from "../audio/controller";
import { LevelController, loadLevelSettings } from "../spectrum/levels";
import type { SocketStatus } from "../spectrum/socket";
import { TuningController } from "../spectrum/tuning";
import { VfoController } from "../vfo/controller";
import { defaultModeFor, snapFrequency, stepFor, type StepChoice } from "../vfo/model";
import { SpectrumMetrics } from "./metrics";

export interface SpectrumInfo {
  centerHz: number;
  sampleRate: number;
  minDb: number;
  maxDb: number;
}

/** Zoom commands the spectrum view exposes to the sidebar. */
export interface ViewActions {
  reset: () => void;
  /** factor < 1 zooms in around the display centre. */
  zoom: (factor: number) => void;
}

const NO_VIEW_ACTIONS: ViewActions = { reset: () => undefined, zoom: () => undefined };

export function useReceiver(device: string) {
  const [levelController] = useState(() => new LevelController(loadLevelSettings()));
  const tuning = useMemo(() => new TuningController({ device }), [device]);
  const tuningSnap = useSyncExternalStore(tuning.subscribe, tuning.getSnapshot);
  const vfoCtl = useMemo(() => new VfoController({ device }), [device]);
  const vfoSnap = useSyncExternalStore(vfoCtl.subscribe, vfoCtl.getSnapshot);
  const audio = useMemo(() => new AudioController({ device }), [device]);
  const audioSnap = useSyncExternalStore(audio.subscribe, audio.getSnapshot);
  const metrics = useMemo(() => new SpectrumMetrics(), []);
  const viewActions = useRef<ViewActions>(NO_VIEW_ACTIONS);
  const registerView = useCallback((actions: ViewActions) => {
    viewActions.current = actions;
  }, []);

  const [status, setStatus] = useState<SocketStatus>("connecting");
  const [info, setInfo] = useState<SpectrumInfo | null>(null);
  const [spectrumError, setSpectrumError] = useState<string | null>(null);
  const [step, setStep] = useState<StepChoice>("auto");

  const stepRef = useRef(step);
  useEffect(() => {
    stepRef.current = step;
  });
  const createVfoAt = useCallback(
    (hz: number | null) => {
      if (hz === null) return;
      const mode = defaultModeFor(hz);
      vfoCtl.create({ freqHz: snapFrequency(hz, stepFor(stepRef.current, mode)), mode });
    },
    [vfoCtl],
  );

  // VFO list polling and the audio socket live as long as the receiver.
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

  const selected = vfoSnap.vfos.find((v) => v.id === vfoSnap.selectedId) ?? null;
  const errors = [spectrumError, tuningSnap.error, vfoSnap.error, audioSnap.error].filter(
    (e): e is string => e !== null,
  );

  return {
    device,
    levelController,
    tuning,
    tuningSnap,
    vfoCtl,
    vfoSnap,
    selected,
    audio,
    audioSnap,
    metrics,
    viewActions,
    registerView,
    status,
    setStatus,
    info,
    setInfo,
    setSpectrumError,
    step,
    setStep,
    createVfoAt,
    errors,
  };
}

export type Receiver = ReturnType<typeof useReceiver>;
