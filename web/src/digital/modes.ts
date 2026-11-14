// Digital modes: the decoder plugins presented like OpenWebRX's "digital modes". A digital mode is
// a bundle of (plugin, VFO settings): picking it sets the demodulator, the bandwidth and an open
// squelch on the VFO and starts the plugin on it; going back to an analog mode stops it. The
// presets live here so the mode bar, the output dock and the tests share one table.

import { AIS_PLUGIN_ID, AIS_VESSEL } from "../ais/format";
import { ACARS_MESSAGE, ACARS_PLUGIN_ID, VDL2_MESSAGE, VDL2_PLUGIN_ID } from "../datalink/model";
import { formatMhz } from "../decoders/events";
import {
  FT8_MESSAGE,
  FT8_PLUGIN_ID,
  WSPR_SPOT,
  presetsOf,
  type DialPreset,
  type Ft8Mode,
} from "../ft8/model";
import type { VfoPatch } from "../vfo/api";
import { DSD_CALL_END, DSD_CALL_START, DSD_PLUGIN_ID } from "../dsd/model";
import { M17_CALL_END, M17_CALL_START, M17_PLUGIN_ID } from "../m17/model";
import { isRec, type Rec } from "../spectrum/api";
import {
  TETRA_CALL_END,
  TETRA_CALL_START,
  TETRA_PLUGIN_ID,
  TETRA_SDS,
  TETRA_STATUS,
} from "../tetra/model";
import type { PluginInstance } from "../plugins/api";

export type DigitalModeId =
  | "pocsag"
  | "aprs"
  | "ism"
  | "m17"
  | "dmr"
  | "dpmr"
  | "nxdn"
  | "tetra"
  | "ft8"
  | "ft4"
  | "wspr"
  | "ais"
  | "acars"
  | "vdl2";

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
  vfo: VfoPatch & { mode: "nfm" | "usb"; bandwidthHz: number; squelchDbfs: null };
  /** Typical frequencies, for the "try this mode here" hint. */
  hints: readonly FrequencyHint[];
  /** Noun for counters: "pages", "packets", "readings", "calls". */
  noun: string;
  /** What to expect while nothing has been decoded yet. */
  expect: string;
  /**
   * Start configuration, for modes that share one plugin (DMR, dPMR and NXDN are one dsd-fme
   * plugin told which protocol to decode; FT8, FT4 and WSPR one plugin told the mode). Also
   * tells the instances of the modes apart.
   */
  config?: Rec;
  /** Dial frequencies of the bands the mode lives on (a band picker next to the mode select). */
  presets?: readonly DialPreset[];
}

// DMR, dPMR and NXDN are one plugin (a user-installed dsd-fme, docs/plugins-dsd.md); the config's
// protocol tells the three modes apart. All three are one 12.5 kHz channel and no calling frequency.
const DSD_TITLE =
  " digital voice (needs your own dsd-fme): source ID, talkgroup, colour code and the audio";
const DSD_EXPECT =
  "Needs dsd-fme, which you install yourself (the panel says when it is missing). Tune exactly onto the channel: a signal a few kHz off, for example from an uncorrected RTL-SDR clock, is lost. Encrypted calls are flagged and not played.";
const dsdMode = (id: "dmr" | "dpmr" | "nxdn", label: string, expect: string): DigitalMode => ({
  id,
  label,
  title: `${label}${DSD_TITLE}`,
  pluginId: DSD_PLUGIN_ID,
  eventKinds: [DSD_CALL_START, DSD_CALL_END],
  vfo: { mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null },
  hints: [],
  noun: "calls",
  expect,
  config: { protocol: id },
});

// FT8, FT4 and WSPR are one plugin (docs/plugins-ft8.md); the config's mode tells them apart. They
// decode the audio of a USB receiver on the dial frequency, in fixed time slots.
const FT8_EXPECT =
  "Stations transmit in fixed 15 second slots, so the first decodes appear up to 30 seconds after the start. The receiver must sit on the dial frequency in USB (pick a band next to the mode) and the clock of this machine must be right (NTP). HF needs an upconverter or direct sampling in front of an RTL-SDR; 2 m (144.174 MHz) works without.";
const ft8Mode = (
  id: Ft8Mode,
  label: string,
  title: string,
  eventKinds: readonly string[],
  noun: string,
  expect: string,
): DigitalMode => ({
  id,
  label,
  title,
  pluginId: FT8_PLUGIN_ID,
  eventKinds,
  vfo: { mode: "usb", bandwidthHz: 3_000, squelchDbfs: null },
  // The dial frequencies of the bands; a VFO on one of them gets "Try ...?".
  hints: presetsOf(id).map((p) => ({
    loHz: p.dialHz - 600,
    hiHz: p.dialHz + 600,
    note: `${label} ${p.band}`,
  })),
  noun,
  expect,
  config: { mode: id },
  presets: presetsOf(id),
});

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
  {
    id: "m17",
    label: "M17",
    title:
      "M17 digital voice (4800 baud 4FSK, Codec2): callsigns, CAN, encryption flag and the audio",
    pluginId: M17_PLUGIN_ID,
    eventKinds: [M17_CALL_START, M17_CALL_END],
    // One 12.5 kHz channel; the receiver's own audio is the 4FSK baseband and is muted while M17 runs.
    vfo: { mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null },
    // M17 has no worldwide calling frequency, so there is nothing to suggest.
    hints: [],
    noun: "calls",
    expect:
      "M17 is used on amateur radio VHF/UHF channels (2 m and 70 cm; check the band plan of your country). Tune exactly onto the channel: a signal a few kHz off, for example from an uncorrected RTL-SDR clock, is lost. The first second of a call may be missing while the decoder locks on.",
  },
  dsdMode("dmr", "DMR", `${DSD_EXPECT} DMR carries two calls at once (time slots 1 and 2).`),
  dsdMode("dpmr", "dPMR", DSD_EXPECT),
  dsdMode("nxdn", "NXDN", `${DSD_EXPECT} The 12.5 kHz variant (NXDN96) is decoded.`),
  {
    id: "tetra",
    label: "TETRA",
    title:
      "TETRA trunked radio, TMO downlink: cell, calls, SSI / GSSI, short messages, encryption flags; voice with your own ETSI codec",
    pluginId: TETRA_PLUGIN_ID,
    eventKinds: [TETRA_CALL_START, TETRA_CALL_END, TETRA_SDS, TETRA_STATUS],
    // One 25 kHz carrier. The plugin takes the VFO's complex baseband; the receiver's own NFM audio
    // is noise and is muted while the decoder runs.
    vfo: { mode: "nfm", bandwidthHz: 25_000, squelchDbfs: null },
    // TETRA sits where the network operator put it; nothing to suggest.
    hints: [],
    noun: "calls and messages",
    expect:
      "Tune onto the downlink carrier of a TETRA cell you may listen to (25 kHz raster; an RTL-SDR with an uncorrected clock can be a few kHz off at 400 MHz, which is the limit of the decoder). The cell shows as soon as it is in sync; voice needs the ETSI speech codec that you install yourself (the panel says when it is missing). Encrypted calls are flagged and never decrypted. Direct mode (DMO) is not decoded.",
  },
  {
    id: "ais",
    label: "AIS",
    title: "AIS ships (161.975 / 162.025 MHz): position, speed, course, name, type on a map",
    pluginId: AIS_PLUGIN_ID,
    eventKinds: [AIS_VESSEL],
    // Both channels (25 kHz apart) fit into 150 kHz around 162.000 MHz; the plugin takes the VFO's
    // complex baseband, the receiver's own NFM audio is noise.
    vfo: { mode: "nfm", bandwidthHz: 150_000, squelchDbfs: null },
    hints: [{ loHz: 161_900_000, hiHz: 162_100_000, note: "AIS (marine, 161.975 / 162.025 MHz)" }],
    noun: "vessels",
    expect:
      "Tune to 162.000 MHz: both AIS channels are inside the 150 kHz receiver. AIS is line of sight, so an antenna with a view of the water matters more than gain; a few ships a minute is normal near a busy lane. Positions appear on the map as soon as a vessel has sent one.",
  },
  {
    id: "acars",
    label: "ACARS",
    title: "ACARS aircraft datalink (VHF, 2400 baud): flight, registration, label and text",
    pluginId: ACARS_PLUGIN_ID,
    eventKinds: [ACARS_MESSAGE],
    // The plugin takes the VFO's complex baseband; the VFO frequency is the channel decoded.
    vfo: { mode: "nfm", bandwidthHz: 200_000, squelchDbfs: null },
    hints: [
      {
        loHz: 131_450_000,
        hiHz: 131_900_000,
        note: "ACARS (131.525 / .725 / .825 MHz Europe, 131.550 MHz North America)",
      },
      {
        loHz: 129_000_000,
        hiHz: 130_500_000,
        note: "ACARS (further channels, mostly North America)",
      },
    ],
    noun: "messages",
    expect:
      "Put the receiver exactly on an ACARS channel (131.725 MHz is busy in Europe); the decoder reads the frequency when it starts, so restart it after retuning. Aircraft send a message every few minutes, so a quiet channel can take a while. Add more channels within 100 kHz in the decoder settings.",
  },
  {
    id: "vdl2",
    label: "VDL2",
    title: "VDL Mode 2 aircraft datalink (136.6 to 137 MHz): ACARS over VDL2, ADS-C, CPDLC",
    pluginId: VDL2_PLUGIN_ID,
    eventKinds: [VDL2_MESSAGE],
    vfo: { mode: "nfm", bandwidthHz: 300_000, squelchDbfs: null },
    hints: [
      {
        loHz: 136_600_000,
        hiHz: 137_000_000,
        note: "VDL Mode 2 (136.975 MHz common signalling channel)",
      },
    ],
    noun: "messages",
    expect:
      "Put the receiver on a VDL2 channel (136.975 MHz is the common signalling channel and always busy near airports; 136.725 to 136.875 MHz carry the traffic in Europe). The decoder reads the frequency when it starts, so restart it after retuning. Link management frames are left out unless you ask for them in the decoder settings.",
  },
  ft8Mode(
    "ft8",
    "FT8",
    "FT8 weak-signal amateur mode (15 s slots): callsigns, locators, SNR",
    [FT8_MESSAGE],
    "messages",
    FT8_EXPECT,
  ),
  ft8Mode(
    "ft4",
    "FT4",
    "FT4 contest mode (7.5 s slots): callsigns, locators, SNR",
    [FT8_MESSAGE],
    "messages",
    FT8_EXPECT.replace("15 second", "7.5 second"),
  ),
  ft8Mode(
    "wspr",
    "WSPR",
    "WSPR propagation beacons (2 min slots): callsign, locator, power, SNR, drift",
    [WSPR_SPOT],
    "spots",
    "Beacons transmit in 2 minute slots, so the first spots appear up to 4 minutes after the start. The receiver must sit on the dial frequency in USB (pick a band next to the mode) and the clock of this machine must be right (NTP). HF needs an upconverter or direct sampling in front of an RTL-SDR; 2 m (144.489 MHz) works without.",
  ),
];

export const digitalMode = (id: DigitalModeId): DigitalMode => {
  const found = DIGITAL_MODES.find((m) => m.id === id);
  if (!found) throw new Error(`unknown digital mode ${id}`);
  return found;
};

export const digitalModeOfPlugin = (pluginId: string): DigitalMode | null =>
  DIGITAL_MODES.find((m) => m.pluginId === pluginId) ?? null;

/**
 * The mode a plugin instance runs: the one of its plugin, and where several modes share a plugin
 * (DMR, dPMR, NXDN; FT8, FT4, WSPR) the one whose config the instance carries; the first of them
 * otherwise.
 */
export function digitalModeOfInstance(instance: PluginInstance): DigitalMode | null {
  const modes = DIGITAL_MODES.filter((m) => m.pluginId === instance.pluginId);
  if (modes.length <= 1) return modes[0] ?? null;
  const config = isRec(instance.config) ? instance.config : {};
  const same = (m: DigitalMode): boolean =>
    m.config !== undefined && Object.entries(m.config).every(([k, v]) => config[k] === v);
  return modes.find(same) ?? modes[0] ?? null;
}

/** Digital modes with a running (or starting) instance on a VFO. */
export function digitalOnVfo(
  instances: readonly PluginInstance[],
  device: string,
  vfo: number,
): { mode: DigitalMode; instance: PluginInstance }[] {
  const out: { mode: DigitalMode; instance: PluginInstance }[] = [];
  for (const instance of instances) {
    if (instance.device !== device || instance.vfo !== vfo) continue;
    const mode = digitalModeOfInstance(instance);
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
