// Entering and leaving a digital mode: VFO preset plus plugin start / stop in one step.

import { useEffect, useRef } from "react";
import { describeHttpError } from "../http";
import { noticeAccepted, rememberNotice } from "../plugins/model";
import type { PluginStore } from "../plugins/store";
import { toasts } from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import { digitalOnVfo, type DigitalMode } from "./modes";

export interface EnterContext {
  store: PluginStore;
  device: string;
  vfo: Vfo;
  updateVfo: (id: number, patch: VfoPatch) => void;
}

export type EnterResult =
  /** The preset is applied and the plugin runs on the VFO. */
  | "started"
  /** The plugin runs already; only the preset was re-applied. */
  | "running"
  /** The legal notice of the plugin has not been confirmed in this browser yet. */
  | "notice";

/**
 * Switch a VFO to a digital mode: other digital decoders on the VFO are stopped, the VFO gets the
 * preset (demodulator, bandwidth, squelch off) and the plugin starts on it. Rejects with the
 * server's message when the plugin cannot start. Resolves with `notice` (and does nothing) when
 * the plugin's notice still needs the user's confirmation; call again with `acknowledged`.
 */
export async function enterDigital(
  ctx: EnterContext,
  mode: DigitalMode,
  acknowledged = false,
): Promise<EnterResult> {
  const { store, device, vfo } = ctx;
  const manifest = store.getSnapshot().plugins.find((p) => p.id === mode.pluginId);
  if (manifest === undefined) {
    throw new Error(`The ${mode.label} decoder plugin is not installed.`);
  }
  if (!acknowledged && !noticeAccepted(manifest)) return "notice";
  const here = digitalOnVfo(store.getSnapshot().instances, device, vfo.id);
  for (const other of here.filter((h) => h.mode.id !== mode.id)) {
    await store.stopInstance(other.instance.id);
  }
  ctx.updateVfo(vfo.id, mode.vfo);
  if (here.some((h) => h.mode.id === mode.id)) return "running";
  await store.startInstance({
    pluginId: mode.pluginId,
    device,
    vfo: vfo.id,
    freqHz: vfo.freqHz,
  });
  rememberNotice(manifest);
  return "started";
}

/** Stop every digital decoder on a VFO (back to an analog receiver). */
export async function leaveDigital(
  store: PluginStore,
  device: string,
  vfoId: number,
): Promise<void> {
  for (const { instance } of digitalOnVfo(store.getSnapshot().instances, device, vfoId)) {
    await store.stopInstance(instance.id);
  }
}

/**
 * Stop the digital decoder of a VFO when the user moves the receiver to another demodulator. A
 * decoder is only watched after the VFO has been seen in NFM with it, so starting a plugin by
 * hand on a WFM receiver (and not applying the suggested settings) is left alone.
 */
export function useLeaveDigitalOnAnalog(
  store: PluginStore,
  device: string,
  vfos: readonly Vfo[],
  instances: ReturnType<PluginStore["getSnapshot"]>["instances"],
): void {
  const armed = useRef(new Set<string>());
  useEffect(() => {
    for (const vfo of vfos) {
      for (const { mode, instance } of digitalOnVfo(instances, device, vfo.id)) {
        if (vfo.mode === "nfm") {
          armed.current.add(instance.id);
        } else if (armed.current.has(instance.id)) {
          armed.current.delete(instance.id);
          void store.stopInstance(instance.id).then(
            () =>
              toasts.push({
                tone: "info",
                title: `${mode.label} stopped`,
                body: `VFO ${vfo.id} is no longer an FM receiver.`,
                key: `digital-left-${instance.id}`,
                durationMs: 4000,
              }),
            (e: unknown) =>
              toasts.push({
                tone: "warn",
                title: `Could not stop ${mode.label}`,
                body: describeHttpError(e),
                key: `digital-left-${instance.id}`,
              }),
          );
        }
      }
    }
    // Forget instances that are gone.
    const live = new Set(instances.map((i) => i.id));
    for (const id of armed.current) if (!live.has(id)) armed.current.delete(id);
  }, [store, device, vfos, instances]);
}
