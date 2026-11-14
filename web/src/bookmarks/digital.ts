// Bridge between bookmarks and the receiver's Digital picker: a running decoder is saved as the
// bookmark's digital channel, and a bookmark with a picker mode starts that mode when it is loaded.

import { DIGITAL_MODES, digitalOnVfo, type DigitalMode } from "../digital/modes";
import type { PluginInstance } from "../plugins/api";
import { formatFrequency } from "../spectrum/freq";
import type { Vfo } from "../vfo/api";
import type { StepChoice } from "../vfo/model";
import {
  PROTOCOL_LABEL,
  newInput,
  receiverState,
  type BookmarkInput,
  type DigitalChannel,
} from "./model";

/** The picker mode a stored protocol id stands for (P25 and unknown ids have none). */
export const modeOfProtocol = (protocol: string): DigitalMode | null =>
  DIGITAL_MODES.find((m) => m.id === protocol) ?? null;

/** The digital channel that records "this mode on this frequency". */
export function channelOf(mode: DigitalMode): DigitalChannel {
  return {
    protocol: mode.id,
    timeslot: null,
    talkgroup: null,
    colourCode: null,
    mcc: null,
    mnc: null,
    decoderPluginId: mode.pluginId,
  };
}

/** The digital mode running on a VFO, as a channel to save with the bookmark; null = analogue. */
export function activeChannel(
  instances: readonly PluginInstance[],
  device: string,
  vfoId: number | null,
): DigitalChannel | null {
  if (vfoId === null) return null;
  const first = digitalOnVfo(instances, device, vfoId)[0];
  return first === undefined ? null : channelOf(first.mode);
}

/**
 * The "add bookmark" draft for the receiver as it is now: the active VFO's frequency, mode,
 * bandwidth, squelch, step and sub-tone, plus the digital mode running on it. The name is the band
 * plan entry's, else the frequency (prefixed with the digital mode's name).
 */
export function draftFor(opts: {
  freqHz: number;
  /** Band plan name of the frequency, "" when it has none. */
  planName: string;
  vfo: Vfo | null;
  squelchIsAuto: boolean;
  step: StepChoice;
  digital: DigitalChannel | null;
}): BookmarkInput {
  const { freqHz, planName, vfo, digital } = opts;
  const mhz = `${formatFrequency(freqHz).slice(0, -4)} MHz`;
  const label = digital === null ? null : PROTOCOL_LABEL[digital.protocol];
  const name =
    planName !== ""
      ? label === null
        ? planName
        : `${planName} (${label})`
      : label === null
        ? mhz
        : `${label} ${mhz}`;
  return newInput({
    name,
    freqHz,
    mode: vfo?.mode ?? null,
    bandwidthHz: vfo?.bandwidthHz ?? null,
    digital,
    ...receiverState(vfo, opts.squelchIsAuto, opts.step),
  });
}
