// Digital modes: the decoder plugins presented like OpenWebRX's "digital modes". A digital mode is
// a bundle of (plugin, VFO settings): picking it sets the demodulator, the bandwidth and an open
// squelch on the VFO and starts the plugin on it; going back to an analog mode stops it. The
// presets live here so the mode bar, the output dock and the tests share one table.

import { formatMhz } from "../decoders/events";
import type { VfoPatch } from "../vfo/api";
import type { PluginInstance } from "../plugins/api";

export type DigitalModeId = "pocsag" | "aprs" | "ism";

export interface FrequencyHint {
  loHz: number;
  hiHz: number;
  /** Where this is, shown in the suggestion tooltip. */
  note: string;
}

export interface DigitalMode {
  id: DigitalModeId;
  /** Short name for the mode bar. */
  label: string;
  /** What it decodes, one line. */
  title: string;
  pluginId: string;
  /** Event types the plugin emits (`pocsag.message`, ...). */
  eventKinds: readonly string[];
  /** The VFO settings that make the plugin work. */
  vfo: VfoPatch & { mode: "nfm"; bandwidthHz: number; squelchDbfs: null };
  /** Typical frequencies, for the "try this mode here" hint. */
  hints: readonly FrequencyHint[];
  /** Noun for counters: "pages", "packets", "readings". */
  noun: string;
  /** What to expect while nothing has been decoded yet. */
  expect: string;
}

export const DIGITAL_MODES: readonly DigitalMode[] = [
  {
    id: "pocsag",
    label: "POCSAG",
    title: "POCSAG / FLEX pagers (512, 1200, 2400 baud, both polarities)",
    pluginId: "org.superhet.decoder.pocsag",
    eventKinds: ["pocsag.message", "flex.message"],
    vfo: { mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null },
    hints: [
      { loHz: 439_975_000, hiHz: 440_000_000, note: "DAPNET (amateur radio paging network)" },
      { loHz: 929_000_000, hiHz: 932_000_000, note: "paging band (Americas)" },
    ],
    noun: "pages",
    expect:
      "Pagers transmit only when somebody is called; on a quiet network that can be minutes. DAPNET sends a time sync about every few minutes.",
  },
  {
    id: "aprs",
    label: "APRS",
    title: "APRS packet radio (1200 baud AFSK)",
    pluginId: "org.superhet.decoder.aprs",
    eventKinds: ["aprs.packet"],
    vfo: { mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null },
    hints: [
      { loHz: 144_790_000, hiHz: 144_810_000, note: "APRS Europe" },
      { loHz: 144_380_000, hiHz: 144_400_000, note: "APRS North America" },
      { loHz: 145_820_000, hiHz: 145_830_000, note: "APRS from the ISS" },
    ],
    noun: "packets",
    expect:
      "APRS is bursty: stations beacon every few minutes, so a packet can take a while. Weak or distant stations may be missed when the antenna or the gain is poor.",
  },
  {
    id: "ism",
    label: "ISM",
    title: "ISM-band sensors (rtl_433): weather stations, thermometers, TPMS, remotes",
    pluginId: "org.superhet.decoder.rtl433",
    eventKinds: ["rtl433.reading"],
    vfo: { mode: "nfm", bandwidthHz: 250_000, squelchDbfs: null },
    hints: [
      { loHz: 433_050_000, hiHz: 434_790_000, note: "433 MHz ISM band" },
      { loHz: 868_000_000, hiHz: 868_600_000, note: "868 MHz ISM band" },
      { loHz: 314_900_000, hiHz: 315_100_000, note: "315 MHz ISM band" },
      { loHz: 915_000_000, hiHz: 928_000_000, note: "915 MHz ISM band" },
    ],
    noun: "readings",
    expect:
      "Most sensors send every 30 to 60 seconds, some only on change (remotes, door contacts). Put the VFO on the sensor's centre frequency, for example 433.92 MHz.",
  },
];

export const digitalMode = (id: DigitalModeId): DigitalMode => {
  const found = DIGITAL_MODES.find((m) => m.id === id);
  if (!found) throw new Error(`unknown digital mode ${id}`);
  return found;
};

export const digitalModeOfPlugin = (pluginId: string): DigitalMode | null =>
  DIGITAL_MODES.find((m) => m.pluginId === pluginId) ?? null;

/** Digital modes with a running (or starting) instance on a VFO. */
export function digitalOnVfo(
  instances: readonly PluginInstance[],
  device: string,
  vfo: number,
): { mode: DigitalMode; instance: PluginInstance }[] {
  const out: { mode: DigitalMode; instance: PluginInstance }[] = [];
  for (const instance of instances) {
    if (instance.device !== device || instance.vfo !== vfo) continue;
    const mode = digitalModeOfPlugin(instance.pluginId);
    if (mode !== null) out.push({ mode, instance });
  }
  return out;
}

/** The digital mode that usually lives on this frequency (a hint, never applied on its own). */
export function suggestDigital(freqHz: number): { mode: DigitalMode; note: string } | null {
  for (const mode of DIGITAL_MODES) {
    const hit = mode.hints.find((h) => freqHz >= h.loHz && freqHz <= h.hiHz);
    if (hit) return { mode, note: hit.note };
  }
  return null;
}

/** Empty-state text of the output panel. */
export function expectText(mode: DigitalMode, freqHz: number | null): string {
  const where = freqHz === null ? "" : ` on ${formatMhz(freqHz)} MHz`;
  return `No ${mode.noun} yet${where}. ${mode.expect}`;
}
