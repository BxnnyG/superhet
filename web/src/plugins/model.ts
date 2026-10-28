// Pure plugin helpers: config forms of the first-party plugins, VFO suggestions derived from the
// manifest, the "notice read" memory and the wording of instance states.

import { formatMhz } from "../decoders/events";
import type { Rec } from "../spectrum/api";
import type { BadgeTone } from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import { MODE_LABEL } from "../vfo/model";
import type { InstanceState, PluginInstance, PluginManifest } from "./api";

/** First-party plugins that ship in the image; the API does not report the origin. */
export const BUNDLED_PLUGIN_IDS: ReadonlySet<string> = new Set([
  "org.superhet.decoder.pocsag",
  "org.superhet.decoder.aprs",
  "org.superhet.decoder.rtl433",
]);

export const isBundled = (id: string): boolean => BUNDLED_PLUGIN_IDS.has(id);

// ---------- instance states ----------

export const STATE_LABEL: Record<InstanceState, string> = {
  starting: "Starting",
  running: "Running",
  backoff: "Restarting",
  error: "Failed",
  stopped: "Stopped",
};

export const STATE_TONE: Record<InstanceState, BadgeTone> = {
  starting: "warn",
  running: "success",
  backoff: "warn",
  error: "danger",
  stopped: "neutral",
};

export const STATE_HINT: Record<InstanceState, string> = {
  starting: "The process is starting",
  running: "Running",
  backoff: "The process ended and is restarted after a short pause",
  error: "Too many crashes in a short time; fix the cause and restart it",
  stopped: "Stopped",
};

/** Where an instance listens: "VFO 2 · 433.92 MHz", "whole device · 1090 MHz". */
export function describeBinding(i: PluginInstance): string {
  const parts: string[] = [];
  if (i.vfo !== null) parts.push(`VFO ${i.vfo}`);
  else if (i.exclusive) parts.push("whole device");
  else if (i.device !== null) parts.push("no input");
  if (i.device !== null) parts.push(i.device);
  if (i.freqHz !== null) parts.push(`${formatMhz(i.freqHz)} MHz`);
  return parts.join(" · ");
}

/** The most urgent problem of an instance, one line; null when it is healthy. */
export function instanceProblem(i: PluginInstance): string | null {
  if (i.lastError !== null && (i.state === "error" || i.state === "backoff")) return i.lastError;
  if (i.inputError !== null) return `Input: ${i.inputError}`;
  return i.lastError;
}

// ---------- notice memory ----------

const ACK_KEY = "superhet.plugin-notices";

function readAcks(): Record<string, string> {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(ACK_KEY) ?? "{}");
    return typeof raw === "object" && raw !== null && !Array.isArray(raw)
      ? (raw as Record<string, string>)
      : {};
  } catch {
    return {};
  }
}

/** Whether this browser confirmed exactly this notice text of the plugin before. */
export function noticeAccepted(m: PluginManifest): boolean {
  if (m.acknowledge === null) return true;
  return readAcks()[m.id] === m.acknowledge.text;
}

export function rememberNotice(m: PluginManifest): void {
  if (m.acknowledge === null) return;
  try {
    localStorage.setItem(ACK_KEY, JSON.stringify({ ...readAcks(), [m.id]: m.acknowledge.text }));
  } catch {
    // Private mode: the notice is asked again next time.
  }
}

// ---------- config forms ----------

export type ConfigField =
  | { type: "bool"; key: string; label: string; hint?: string; default: boolean }
  | { type: "multi"; key: string; label: string; hint?: string; options: readonly number[] }
  | {
      type: "select";
      key: string;
      label: string;
      hint?: string;
      options: readonly { value: string; label: string }[];
      /** Config value per option, for options whose value is not the option string itself. */
      values?: Readonly<Record<string, string | boolean>>;
    }
  | { type: "numlist"; key: string; label: string; hint?: string; placeholder?: string };

/** Config keys of the first-party plugins (docs/plugins-decoders.md); others get a JSON field. */
export const CONFIG_FIELDS: Readonly<Record<string, readonly ConfigField[]>> = {
  "org.superhet.decoder.pocsag": [
    {
      type: "multi",
      key: "baud",
      label: "Baud rates",
      hint: "None selected decodes 512, 1200 and 2400.",
      options: [512, 1200, 2400],
    },
    { type: "bool", key: "flex", label: "Decode FLEX as well", default: true },
    {
      type: "select",
      key: "invert",
      label: "Signal polarity",
      hint: "Automatic runs a normal and an inverted decoder side by side; pick one only to save CPU.",
      options: [
        { value: "", label: "Automatic (both)" },
        { value: "normal", label: "Normal" },
        { value: "inverted", label: "Inverted" },
      ],
      values: { normal: false, inverted: true },
    },
    {
      type: "select",
      key: "charset",
      label: "National characters",
      hint: "Umlauts and accents in alphanumeric pages.",
      options: [
        { value: "", label: "Default" },
        { value: "US", label: "US" },
        { value: "DE", label: "DE" },
        { value: "FR", label: "FR" },
        { value: "DK", label: "DK" },
        { value: "SE", label: "SE" },
        { value: "SI", label: "SI" },
      ],
    },
  ],
  "org.superhet.decoder.aprs": [],
  "org.superhet.decoder.rtl433": [
    {
      type: "numlist",
      key: "protocols",
      label: "Protocols",
      hint: "rtl_433 protocol numbers (rtl_433 -R help), comma separated. Empty uses its standard set; a short list keeps the history quiet.",
      placeholder: "e.g. 19, 20",
    },
  ],
};

export type ConfigValue = boolean | readonly number[] | string;
export type ConfigDraft = Record<string, ConfigValue>;

/** Form state with every field at its default. */
export function defaultDraft(fields: readonly ConfigField[]): ConfigDraft {
  const draft: ConfigDraft = {};
  for (const f of fields) {
    draft[f.key] = f.type === "bool" ? f.default : f.type === "multi" ? [] : "";
  }
  return draft;
}

/** The form state of an existing config (the inverse of [`buildConfig`]); unknown keys are ignored. */
export function configToDraft(fields: readonly ConfigField[], config: unknown): ConfigDraft {
  const draft = defaultDraft(fields);
  if (typeof config !== "object" || config === null || Array.isArray(config)) return draft;
  const c = config as Rec;
  for (const f of fields) {
    const v = c[f.key];
    if (v === undefined || v === null) continue;
    if (f.type === "bool") {
      if (typeof v === "boolean") draft[f.key] = v;
    } else if (f.type === "multi") {
      if (Array.isArray(v)) draft[f.key] = v.filter((x): x is number => typeof x === "number");
    } else if (f.type === "select") {
      const mapped = f.values
        ? Object.entries(f.values).find(([, value]) => value === v)?.[0]
        : typeof v === "string"
          ? v
          : undefined;
      if (mapped !== undefined) draft[f.key] = mapped;
    } else if (Array.isArray(v)) {
      draft[f.key] = v.join(", ");
    }
  }
  return draft;
}

export type BuiltConfig = { ok: true; config: Rec } | { ok: false; error: string };

/** The config object of a draft: only values that differ from the plugin's defaults. */
export function buildConfig(fields: readonly ConfigField[], draft: ConfigDraft): BuiltConfig {
  const config: Rec = {};
  for (const f of fields) {
    const v = draft[f.key];
    if (f.type === "bool") {
      if (typeof v === "boolean" && v !== f.default) config[f.key] = v;
    } else if (f.type === "multi") {
      if (Array.isArray(v) && v.length > 0 && v.length < f.options.length) {
        config[f.key] = [...v].sort((a, b) => a - b);
      }
    } else if (f.type === "select") {
      if (typeof v === "string" && v !== "") config[f.key] = f.values?.[v] ?? v;
    } else if (typeof v === "string" && v.trim() !== "") {
      const items = v.split(/[\s,;]+/).filter((s) => s !== "");
      const nums = items.map((s) => (/^\d{1,5}$/.test(s) ? Number(s) : NaN));
      if (nums.some((n) => !Number.isInteger(n))) {
        return { ok: false, error: `${f.label}: use whole numbers separated by commas.` };
      }
      config[f.key] = nums;
    }
  }
  return { ok: true, config };
}

/** Parse the free-form JSON config of a plugin without a form. */
export function parseConfigJson(text: string): BuiltConfig {
  if (text.trim() === "") return { ok: true, config: {} };
  try {
    const v: unknown = JSON.parse(text);
    if (typeof v !== "object" || v === null || Array.isArray(v)) {
      return { ok: false, error: "The config must be a JSON object." };
    }
    return { ok: true, config: v as Rec };
  } catch {
    return { ok: false, error: "The config is not valid JSON." };
  }
}

// ---------- VFO suggestions ----------

export interface Suggestion {
  patch: VfoPatch;
  /** What the change does and why, one line each. */
  lines: string[];
}

const kHz = (hz: number) => `${parseFloat((hz / 1000).toFixed(2))} kHz`;

/**
 * Settings the VFO should have for a plugin, from its manifest: the channel width it declares
 * (rtl_433 wants about 250 kHz of baseband), an NFM receiver for discriminator decoders and an
 * open squelch for data channels. Null when the VFO already fits or the plugin takes no VFO.
 */
export function suggestVfo(m: PluginManifest, v: Vfo): Suggestion | null {
  const input = m.input;
  if (input === null || m.device.exclusive) return null;
  const patch: VfoPatch = {};
  const lines: string[] = [];

  if (input.kind === "discriminator" && v.mode !== "nfm") {
    patch.mode = "nfm";
    lines.push(`Mode ${MODE_LABEL[v.mode]} → NFM: the decoder needs FM-demodulated audio.`);
  }
  const want = input.bandwidthHz;
  if (want !== null) {
    const iq = input.kind === "iq";
    const changed = patch.mode !== undefined;
    const off = iq
      ? v.bandwidthHz < want * 0.9
      : v.bandwidthHz > want * 2 || v.bandwidthHz < want / 2;
    if ((changed || off) && v.bandwidthHz !== want) {
      patch.bandwidthHz = want;
      lines.push(
        iq
          ? `Bandwidth ${kHz(v.bandwidthHz)} → ${kHz(want)}: the decoder needs the whole signal; a narrow VFO misses wideband sensors.`
          : `Bandwidth ${kHz(v.bandwidthHz)} → ${kHz(want)}: the channel width the decoder expects.`,
      );
    }
  }
  if (input.kind === "discriminator" && v.squelchDbfs !== null) {
    patch.squelchDbfs = null;
    lines.push("Squelch off: data bursts that start below the threshold would be cut.");
  }
  return lines.length === 0 ? null : { patch, lines };
}

/** Short input description for lists: "IQ 250 kHz cu8", "discriminator 22.05 kHz s16le". */
export function describeInput(m: PluginManifest): string {
  if (m.input === null) return "no input";
  const rate = m.input.sampleRate;
  const khz = rate >= 1000 ? `${parseFloat((rate / 1000).toFixed(3))} kHz` : `${rate} Hz`;
  const kind = m.input.kind === "iq" ? "IQ" : m.input.kind;
  return `${kind} ${khz} ${m.input.format}`;
}

/** Frequency range text of an exclusive plugin, or null. */
export function describeClaim(m: PluginManifest): string | null {
  const d = m.device;
  if (!d.exclusive) return null;
  if (d.centerHz !== null) return `${formatMhz(d.centerHz)} MHz`;
  if (d.centerHzMin !== null && d.centerHzMax !== null) {
    return `${formatMhz(d.centerHzMin)} to ${formatMhz(d.centerHzMax)} MHz`;
  }
  return null;
}
