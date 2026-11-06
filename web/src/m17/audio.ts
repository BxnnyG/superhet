// Digital voice audio routing (M17, and DMR / dPMR / NXDN through dsd-fme): the decoded voice is the audio of a pseudo VFO (the plugin instance), and the
// receiver the plugin is bound to would only play the raw 4FSK baseband, so it is muted while the
// decoder runs.

import { useEffect, useMemo, useRef } from "react";
import type { AudioController } from "../audio/controller";
import type { PluginInstance } from "../plugins/api";
import { DSD_PLUGIN_ID } from "../dsd/model";
import { M17_PLUGIN_ID } from "./model";

/** Plugins whose decoded voice is a pseudo VFO and whose receiver would only play the baseband. */
const VOICE_PLUGINS: ReadonlySet<string> = new Set([M17_PLUGIN_ID, DSD_PLUGIN_ID]);

export interface M17Routes {
  /** Pseudo VFO ids carrying decoded voice. */
  audioVfos: number[];
  /** Receivers (VFO ids) an M17 decoder is bound to. */
  receivers: number[];
}

export function m17Routes(instances: readonly PluginInstance[], device: string): M17Routes {
  const audioVfos: number[] = [];
  const receivers: number[] = [];
  for (const i of instances) {
    if (!VOICE_PLUGINS.has(i.pluginId) || i.device !== device) continue;
    if (i.audioVfo !== null) audioVfos.push(i.audioVfo);
    if (i.vfo !== null) receivers.push(i.vfo);
  }
  return { audioVfos, receivers };
}

/**
 * Routes of the M17 decoders on `device`, with a stable identity while the sets do not change,
 * and the muting of the bound receivers: muted when a decoder appears, listened to again when it
 * goes (only the receivers this hook muted).
 */
export function useM17Audio(
  audio: AudioController,
  instances: readonly PluginInstance[],
  device: string,
): M17Routes {
  const routes = m17Routes(instances, device);
  const key = `${routes.audioVfos.join(",")}|${routes.receivers.join(",")}`;
  // eslint-disable-next-line react-hooks/exhaustive-deps -- `key` stands for the contents
  const stable = useMemo(() => routes, [key]);
  const muted = useRef(new Set<number>());
  useEffect(() => {
    const wanted = new Set(stable.receivers);
    for (const id of wanted) {
      if (muted.current.has(id)) continue;
      muted.current.add(id);
      audio.setListening(id, false);
    }
    for (const id of [...muted.current]) {
      if (wanted.has(id)) continue;
      muted.current.delete(id);
      audio.setListening(id, true);
    }
  }, [audio, stable]);
  return stable;
}
