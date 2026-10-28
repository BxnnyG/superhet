// Owns the receiver controllers (tuning, VFOs, audio, display levels) and the coarse UI state
// derived from them. Per-frame work stays inside the controllers and canvas renderers.

import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { AudioController } from "../audio/controller";
import { usePermissions } from "../auth/context";
import {
  NEEDS_OPERATOR,
  canListenVfo,
  canModifyVfo,
  frequencyAllowed,
  knownVfoLimit,
  modeAllowed,
  ownVfoCount,
  type Permissions,
} from "../auth/permissions";
import { builtinBandPlan, matchBandPlan, suggestName } from "../bandplan/model";
import { OverlayStore } from "../bandplan/settings";
import { newInput, receiverState } from "../bookmarks/model";
import { BookmarkStore } from "../bookmarks/store";
import { ADSB_PLUGIN_ID } from "../adsb/model";
import { AdsbStore } from "../adsb/store";
import { DAB_PLUGIN_ID } from "../dab/model";
import { DabStore } from "../dab/store";
import { channelTarget, matchChannel, stepChannelBy } from "../channels/lookup";
import { modeOf, type Plan, type PlanChannel } from "../channels/schema";
import { ChannelStore } from "../channels/store";
import { DecoderEventStore } from "../decoders/store";
import { claimOwner } from "../plugins/claim";
import { PluginStore } from "../plugins/store";
import { createProfileApi } from "../profiles/api";
import type { StateDoc } from "../state/doc";
import { bindState, StateStore } from "../state/store";
import type { AnnotationTarget } from "../spectrum/annotations";
import { formatFrequency } from "../spectrum/freq";
import { LevelController, loadLevelSettings } from "../spectrum/levels";
import type { SocketStatus } from "../spectrum/socket";
import { TuningController } from "../spectrum/tuning";
import { toasts } from "../ui/toast";
import { AutoSquelch } from "../vfo/autoSquelch";
import { VfoController } from "../vfo/controller";
import { defaultModeFor, snapFrequency, STEPS_HZ, stepFor, type StepChoice } from "../vfo/model";
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

/** Why a new VFO must not even be requested (guest policy and quota known up front), or null. */
export function createBlockReason(
  perms: Permissions,
  vfos: readonly { mine?: boolean }[],
  req: { freqHz: number; mode: string },
): string | null {
  const limit = knownVfoLimit(perms);
  if (limit !== null && ownVfoCount(vfos) >= limit) {
    return `You can open at most ${limit} ${limit === 1 ? "receiver" : "receivers"}. Close one first.`;
  }
  if (!modeAllowed(perms, req.mode)) return "This mode is not open to guests.";
  if (!frequencyAllowed(perms, req.freqHz))
    return "This frequency is outside the range open to guests.";
  return null;
}

export function useReceiver(device: string) {
  const perms = usePermissions();
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
  const canTune = perms.canTune;
  useEffect(() => tuning.setAllowed(canTune), [tuning, canTune]);
  // Role, ownership and guest limits: refused changes are not sent and say why.
  useEffect(() => {
    vfoCtl.setGuards({
      canModify: (vfo) => canModifyVfo(perms, vfo),
      createBlock: (req, current) => createBlockReason(perms, current, req),
    });
  }, [vfoCtl, perms]);
  const vfoSnap = useSyncExternalStore(vfoCtl.subscribe, vfoCtl.getSnapshot);
  const audio = useMemo(() => new AudioController({ device }), [device]);
  const [autoSquelch] = useState(() => new AutoSquelch());
  const audioSnap = useSyncExternalStore(audio.subscribe, audio.getSnapshot);
  const metrics = useMemo(() => new SpectrumMetrics(), []);
  const viewActions = useRef<ViewActions>(NO_VIEW_ACTIONS);
  const registerView = useCallback((actions: ViewActions) => {
    viewActions.current = actions;
  }, []);

  const [overlay] = useState(() => new OverlayStore());
  const decoderEvents = useMemo(() => new DecoderEventStore({ device }), [device]);
  const plugins = useMemo(() => new PluginStore(), []);
  const pluginSnap = useSyncExternalStore(plugins.subscribe, plugins.getSnapshot);
  const dab = useMemo(() => new DabStore(device), [device]);
  const dabSnap = useSyncExternalStore(dab.subscribe, dab.getSnapshot);
  const adsb = useMemo(() => new AdsbStore(), []);
  const bookmarks = useMemo(() => new BookmarkStore(), []);
  const bookmarkSnap = useSyncExternalStore(bookmarks.subscribe, bookmarks.getSnapshot);

  const channels = useMemo(() => new ChannelStore(), []);
  const channelSnap = useSyncExternalStore(channels.subscribe, channels.getSnapshot);

  const [status, setStatus] = useState<SocketStatus>("connecting");
  const [info, setInfo] = useState<SpectrumInfo | null>(null);
  const [spectrumError, setSpectrumError] = useState<string | null>(null);
  const [step, setStep] = useState<StepChoice>("auto");
  const canSeeDecoders = perms.canSeeDecoders;
  const canUsePlugins = perms.canUsePlugins;

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

  // A band plan channel or bookmark tunes the active VFO (mode, bandwidth, squelch and tone when it
  // has them, and the step); with no VFO yet it creates one. Alt-click on a bookmark makes a new VFO.
  const applyStep = useCallback((t: AnnotationTarget) => {
    if (t.stepHz !== undefined && (STEPS_HZ as readonly number[]).includes(t.stepHz)) {
      setStep(t.stepHz);
    }
  }, []);
  const tuneTo = useCallback(
    (t: AnnotationTarget) => {
      const snap = vfoCtl.getSnapshot();
      const vfo = snap.vfos.find((v) => v.id === snap.selectedId) ?? snap.vfos[0];
      if (vfo) vfoCtl.update(vfo.id, patchFor(vfo, t));
      else if (snap.loaded) vfoCtl.create(createFor(t));
      applyStep(t);
    },
    [vfoCtl, applyStep],
  );
  const tuneNew = useCallback(
    (t: AnnotationTarget) => {
      vfoCtl.create(createFor(t));
      applyStep(t);
    },
    [vfoCtl, applyStep],
  );

  // A channel of a plan: voice channels tune a VFO like a bookmark; a DAB block moves the device
  // centre (operators) to the block centre with a sample rate that covers the 1.536 MHz ensemble.
  const tuneChannel = useCallback(
    (plan: Plan, channel: PlanChannel, newVfo: boolean) => {
      if (modeOf(plan, channel) !== "dab") {
        const target = channelTarget(plan, channel);
        if (target === null) return;
        if (newVfo) tuneNew(target);
        else tuneTo(target);
        return;
      }
      if (!canTune) {
        toasts.push({
          key: "tune-denied",
          tone: "warn",
          title: "Operators and admins only",
          body: "Only operators and admins can move the device centre to a DAB block.",
        });
        return;
      }
      // The DAB plugin takes the device: an instance that runs follows the click (new block),
      // else the DAB tab asks for the takeover first.
      if (dab.getSnapshot().instanceId !== null) {
        dab.control({ cmd: "tune", channel: channel.number }).catch((e: unknown) =>
          toasts.push({
            key: "dab-tune",
            tone: "warn",
            title: "Could not tune DAB",
            body: e instanceof Error ? e.message : String(e),
          }),
        );
      } else {
        dab.request({ channel: channel.number, freqHz: channel.freqHz });
      }
    },
    [canTune, dab, tuneTo, tuneNew],
  );

  // The < > buttons and the arrow keys: when the active VFO sits on a plan channel and channel
  // stepping is on, they step channels. False = not handled (step by frequency).
  const stepChannels = useCallback(
    (steps: number): boolean => {
      const cs = channels.getSnapshot();
      if (!cs.prefs.stepByChannel) return false;
      const snap = vfoCtl.getSnapshot();
      const vfo = snap.vfos.find((v) => v.id === snap.selectedId);
      if (!vfo) return false;
      const match = matchChannel(cs.index, vfo.freqHz, {
        region: cs.region,
        rasters: cs.prefs.rasters,
      });
      if (!match) return false;
      const next = stepChannelBy(match, steps);
      if (next === null) {
        toasts.push({
          key: "channel-end",
          tone: "info",
          title: `${steps < 0 ? "First" : "Last"} channel of ${match.plan.name}`,
          body: "Turn channel stepping off in the Channels tab to step by frequency here.",
        });
        return true;
      }
      const tone = next.channel.ctcssHz;
      const toneOk = vfo.mode === "nfm" || vfo.mode === "am";
      vfoCtl.update(vfo.id, {
        freqHz: next.channel.freqHz,
        ...(tone !== null && toneOk ? { toneSquelchHz: tone } : {}),
      });
      return true;
    },
    [channels, vfoCtl],
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
    // Guests only get the decoder history while the policy shows it.
    if (canSeeDecoders) decoderEvents.start();
    if (canSeeDecoders) dab.start();
    if (canSeeDecoders) adsb.start();
    if (canUsePlugins) plugins.start();
    return () => {
      dab.stop();
      adsb.stop();
      plugins.stop();
      unbind();
      store.stop();
      vfoCtl.stop();
      audio.stop();
      decoderEvents.stop();
    };
  }, [
    store,
    tuning,
    vfoCtl,
    audio,
    decoderEvents,
    canSeeDecoders,
    plugins,
    canUsePlugins,
    dab,
    adsb,
  ]);

  // Bookmarks live on the server: read them at start and again once the state feed is up.
  const feedUp = feed.status === "connected";
  const canReadBookmarks = perms.canReadBookmarks;
  useEffect(() => {
    if (canReadBookmarks) void bookmarks.refresh();
  }, [bookmarks, feedUp, canReadBookmarks]);
  // Custom channel plans live on the server too; the built-in ones need no request.
  useEffect(() => {
    void channels.refresh();
  }, [channels, feedUp]);

  // The audio controller subscribes to the listened VFOs plus the active one (its S-meter and
  // squelch Auto need frames even when it is not in the mix). Wait for the first VFO list so
  // saved listen settings are not pruned against an empty one.
  // Only receivers the viewer may hear: one foreign id makes the whole subscription `forbidden`.
  const vfoIds = vfoSnap.vfos
    .filter((v) => canListenVfo(perms, v))
    .map((v) => v.id)
    .join(",");
  const vfosLoaded = vfoSnap.loaded;
  // The audio of a running DAB plugin is one more stream on the same socket (pseudo VFO id).
  const dabAudioVfo = dabSnap.reply?.audioVfo ?? null;
  useEffect(() => {
    if (!vfosLoaded) return;
    const ids = vfoIds === "" ? [] : vfoIds.split(",").map(Number);
    audio.setVfos(dabAudioVfo === null ? ids : [...ids, dabAudioVfo]);
  }, [audio, vfoIds, vfosLoaded, dabAudioVfo]);
  useEffect(() => {
    audio.setActive(vfoSnap.selectedId);
  }, [audio, vfoSnap.selectedId]);
  // A VFO that moves to another station must not keep showing the old station's RDS data.
  useEffect(() => {
    for (const v of vfoSnap.vfos) audio.noteFrequency(v.id, v.freqHz);
  }, [audio, vfoSnap.vfos]);

  // The dock always edits one VFO: keep the first selected whenever VFOs exist.
  const firstVfoId = vfoSnap.vfos[0]?.id ?? null;
  const hasSelection = vfoSnap.selectedId !== null;
  useEffect(() => {
    if (firstVfoId !== null && !hasSelection) vfoCtl.select(firstVfoId);
  }, [vfoCtl, firstVfoId, hasSelection]);

  const locks = tuningSnap.state?.locks ?? [];
  // The DAB instance to follow: operators know it from the instance list, everybody else from the
  // plugin's claim on the device (a `claim` holder is named by the instance id).
  const dabInstance = pluginSnap.instances.find(
    (i) => i.pluginId === DAB_PLUGIN_ID && i.device === device && i.state !== "stopped",
  )?.id;
  const claimId = locks.find((l) => l.kind === "claim")?.id;
  const followId = dabInstance ?? claimId ?? null;
  useEffect(() => dab.follow(followId), [dab, followId]);
  // The same for ADS-B; each view ignores the other plugin's instance.
  const adsbInstance = pluginSnap.instances.find(
    (i) => i.pluginId === ADSB_PLUGIN_ID && i.device === device && i.state !== "stopped",
  )?.id;
  const adsbFollowId = adsbInstance ?? claimId ?? null;
  useEffect(() => adsb.follow(adsbFollowId), [adsb, adsbFollowId]);
  // Who owns the device while a claim is held: the tuning strip says so instead of showing a
  // VFO that merely sits at its old frequency. Non-operators only learn it for DAB.
  const claim = claimOwner(locks, pluginSnap.instances, pluginSnap.plugins, [
    { instanceId: dabSnap.instanceId, pluginId: dabSnap.reply?.pluginId ?? null },
  ]);
  const profileApi = useMemo(() => createProfileApi(), []);
  const currentDoc = useMemo<StateDoc | null>(
    () =>
      tuningSnap.state && vfoSnap.loaded ? { device: tuningSnap.state, vfos: vfoSnap.vfos } : null,
    [tuningSnap.state, vfoSnap.loaded, vfoSnap.vfos],
  );
  const applyDoc = useCallback((doc: StateDoc) => store.ingest(doc), [store]);
  const unlock = useCallback(() => tuning.setLock(false), [tuning]);
  const toggleLock = useCallback(
    (on: boolean, force = false) => {
      void tuning.setLock(on, force);
    },
    [tuning],
  );

  const selected = vfoSnap.vfos.find((v) => v.id === vfoSnap.selectedId) ?? null;
  const centerHz = tuningSnap.display?.centerHz ?? null;
  // The plan channel the active VFO sits on (drives the readout chip and channel stepping).
  const channelMatch = useMemo(
    () =>
      selected === null
        ? null
        : matchChannel(channelSnap.index, selected.freqHz, {
            region: channelSnap.region,
            rasters: channelSnap.prefs.rasters,
          }),
    [selected, channelSnap],
  );

  // Open the bookmark dialog for the active VFO (or the centre), named after the band plan entry.
  const canEditBookmarks = perms.canEditBookmarks;
  const startBookmark = useCallback(() => {
    if (!canEditBookmarks) {
      toasts.push({
        key: "bookmark-denied",
        tone: "warn",
        title: NEEDS_OPERATOR,
        body: "Bookmarks can only be added by operators and admins.",
      });
      return;
    }
    const freqHz = selected?.freqHz ?? centerHz;
    if (freqHz === null) return;
    const name = suggestName(matchBandPlan(builtinBandPlan(), freqHz));
    bookmarks.openEditor({
      kind: "create",
      draft: newInput({
        name: name || `${formatFrequency(freqHz).slice(0, -4)} MHz`,
        freqHz,
        mode: selected?.mode ?? null,
        bandwidthHz: selected?.bandwidthHz ?? null,
        ...receiverState(selected, selected !== null && autoSquelch.isAuto(selected), step),
      }),
    });
  }, [bookmarks, selected, centerHz, autoSquelch, step, canEditBookmarks]);
  const errors = [spectrumError, tuningSnap.error, vfoSnap.error, audioSnap.error].filter(
    (e): e is string => e !== null,
  );

  return {
    device,
    store,
    feed,
    locks,
    claim,
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
    autoSquelch,
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
    channels,
    channelSnap,
    channelMatch,
    tuneChannel,
    stepChannels,
    decoderEvents,
    plugins,
    dab,
    dabSnap,
    adsb,
    tuneTo,
    tuneNew,
    startBookmark,
    errors,
  };
}

export type Receiver = ReturnType<typeof useReceiver>;
