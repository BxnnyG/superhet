// Owns the receiver controllers (tuning, VFOs, audio, display levels) and the coarse UI state
// derived from them. Per-frame work stays inside the controllers and canvas renderers.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { AudioController } from "../audio/controller";
import { builtinBandPlan, matchBandPlan, suggestName } from "../bandplan/model";
import { OverlayStore } from "../bandplan/settings";
import { BookmarkStore } from "../bookmarks/store";
import { createProfileApi } from "../profiles/api";
import type { StateDoc } from "../state/doc";
import { bindState, StateStore } from "../state/store";
import type { AnnotationTarget } from "../spectrum/annotations";
import { formatFrequency } from "../spectrum/freq";
import { LevelController, loadLevelSettings } from "../spectrum/levels";
import type { SocketStatus } from "../spectrum/socket";
import { TuningController } from "../spectrum/tuning";
import { VfoController } from "../vfo/controller";
import { defaultModeFor, snapFrequency, stepFor, type StepChoice } from "../vfo/model";
import { SpectrumMetrics } from "./metrics";
import { createFor, patchFor } from "./annotationTuning";
import { noticeLocked, noticeRetune } from "./notices";
import { spectrumTuningView } from "./spectrumTuning";

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
  const store = useMemo(() => new StateStore({ device }), [device]);
  const feed = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const tuning = useMemo(
    () => new TuningController({ device, onLocked: (holders) => noticeLocked(holders) }),
    [device],
  );
  const tuningSnap = useSyncExternalStore(tuning.subscribe, tuning.getSnapshot);
  const vfoCtl = useMemo(
    () =>
      new VfoController({
        device,
        onRetune: noticeRetune,
        onState: (state) => tuning.ingest(state),
      }),
    [device, tuning],
  );
  const vfoSnap = useSyncExternalStore(vfoCtl.subscribe, vfoCtl.getSnapshot);
  const audio = useMemo(() => new AudioController({ device }), [device]);
  const audioSnap = useSyncExternalStore(audio.subscribe, audio.getSnapshot);
  const metrics = useMemo(() => new SpectrumMetrics(), []);
  const viewActions = useRef<ViewActions>(NO_VIEW_ACTIONS);
  const registerView = useCallback((actions: ViewActions) => {
    viewActions.current = actions;
  }, []);

  const [overlay] = useState(() => new OverlayStore());
  const bookmarks = useMemo(() => new BookmarkStore(), []);
  const bookmarkSnap = useSyncExternalStore(bookmarks.subscribe, bookmarks.getSnapshot);

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

  // A band plan channel or bookmark tunes the active VFO (mode and bandwidth when it has them);
  // with no VFO yet it creates one.
  const tuneTo = useCallback(
    (t: AnnotationTarget) => {
      const snap = vfoCtl.getSnapshot();
      const vfo = snap.vfos.find((v) => v.id === snap.selectedId) ?? snap.vfos[0];
      if (vfo) vfoCtl.update(vfo.id, patchFor(vfo, t));
      else if (snap.loaded) vfoCtl.create(createFor(t));
    },
    [vfoCtl],
  );

  // A plain click on the spectrum creates the first VFO instead of moving the device centre.
  const spectrumTuning = useMemo(
    // eslint-disable-next-line react-hooks/refs -- createVfoAt only reads its ref when called
    () => spectrumTuningView(tuning, vfoCtl, createVfoAt),
    [tuning, vfoCtl, createVfoAt],
  );

  // The state socket feeds the tuning and VFO controllers; the initial REST reads they make at
  // start are the fallback if it is not up yet. The audio socket lives as long as the receiver.
  useEffect(() => {
    const unbind = bindState(store, { tuning, vfos: vfoCtl });
    store.start();
    vfoCtl.start();
    audio.start();
    return () => {
      unbind();
      store.stop();
      vfoCtl.stop();
      audio.stop();
    };
  }, [store, tuning, vfoCtl, audio]);

  // Bookmarks live on the server: read them at start and again once the state feed is up.
  const feedUp = feed.status === "connected";
  useEffect(() => {
    void bookmarks.refresh();
  }, [bookmarks, feedUp]);

  // The audio controller subscribes to the listened VFOs plus the active one (its S-meter and
  // squelch Auto need frames even when it is not in the mix). Wait for the first VFO list so
  // saved listen settings are not pruned against an empty one.
  const vfoIds = vfoSnap.vfos.map((v) => v.id).join(",");
  const vfosLoaded = vfoSnap.loaded;
  useEffect(() => {
    if (!vfosLoaded) return;
    audio.setVfos(vfoIds === "" ? [] : vfoIds.split(",").map(Number));
  }, [audio, vfoIds, vfosLoaded]);
  useEffect(() => {
    audio.setActive(vfoSnap.selectedId);
  }, [audio, vfoSnap.selectedId]);

  // The dock always edits one VFO: keep the first selected whenever VFOs exist.
  const firstVfoId = vfoSnap.vfos[0]?.id ?? null;
  const hasSelection = vfoSnap.selectedId !== null;
  useEffect(() => {
    if (firstVfoId !== null && !hasSelection) vfoCtl.select(firstVfoId);
  }, [vfoCtl, firstVfoId, hasSelection]);

  const locks = tuningSnap.state?.locks ?? [];
  const profileApi = useMemo(() => createProfileApi(), []);
  const currentDoc = useMemo<StateDoc | null>(
    () =>
      tuningSnap.state && vfoSnap.loaded ? { device: tuningSnap.state, vfos: vfoSnap.vfos } : null,
    [tuningSnap.state, vfoSnap.loaded, vfoSnap.vfos],
  );
  const applyDoc = useCallback((doc: StateDoc) => store.ingest(doc), [store]);
  const unlock = useCallback(() => tuning.setLock(false), [tuning]);
  const toggleLock = useCallback(
    (on: boolean) => {
      void tuning.setLock(on);
    },
    [tuning],
  );

  const selected = vfoSnap.vfos.find((v) => v.id === vfoSnap.selectedId) ?? null;
  const centerHz = tuningSnap.display?.centerHz ?? null;

  // Open the bookmark dialog for the active VFO (or the centre), named after the band plan entry.
  const startBookmark = useCallback(() => {
    const freqHz = selected?.freqHz ?? centerHz;
    if (freqHz === null) return;
    const name = suggestName(matchBandPlan(builtinBandPlan(), freqHz));
    bookmarks.openEditor({
      kind: "create",
      draft: {
        name: name || `${formatFrequency(freqHz).slice(0, -4)} MHz`,
        freqHz,
        mode: selected?.mode ?? null,
        bandwidthHz: selected?.bandwidthHz ?? null,
        tags: [],
        color: null,
        notes: null,
      },
    });
  }, [bookmarks, selected, centerHz]);
  const errors = [spectrumError, tuningSnap.error, vfoSnap.error, audioSnap.error].filter(
    (e): e is string => e !== null,
  );

  return {
    device,
    store,
    feed,
    locks,
    toggleLock,
    profileApi,
    currentDoc,
    applyDoc,
    unlock,
    levelController,
    tuning,
    spectrumTuning,
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
    overlay,
    bookmarks,
    bookmarkSnap,
    tuneTo,
    startBookmark,
    errors,
  };
}

export type Receiver = ReturnType<typeof useReceiver>;
