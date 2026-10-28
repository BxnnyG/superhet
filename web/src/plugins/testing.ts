// Fixtures for the plugin tests: manifests as the API sends them and a fake plugin API.

import { vi } from "vitest";
import type { Rec } from "../spectrum/api";
import {
  parseInstance,
  parseManifest,
  type PluginApi,
  type PluginInstance,
  type PluginManifest,
} from "./api";

export const POCSAG_ID = "org.superhet.decoder.pocsag";
export const RTL433_ID = "org.superhet.decoder.rtl433";
export const APRS_ID = "org.superhet.decoder.aprs";
export const ADSB_ID = "org.example.decoder.adsb";

/** Manifest JSON of the first-party POCSAG plugin (plugins/pocsag/plugin.toml). */
export const POCSAG_RAW = {
  plugin: {
    id: POCSAG_ID,
    name: "POCSAG / FLEX pagers",
    version: "0.1.0",
    protocol: 1,
    license: "AGPL-3.0-only",
    type: "decoder",
    description: "Decodes POCSAG and FLEX pager traffic",
    homepage: "https://github.com/BxnnyG/superhet",
  },
  runtime: { kind: "process", transport: "simple", command: ["superhet-plugin-adapter", "pocsag"] },
  input: { kind: "discriminator", sample_rate: 22050, format: "s16le", bandwidth_hz: 12500 },
  device: { exclusive: false },
  output: { events_schema: "schema/events.json" },
  install: {
    acknowledge: {
      text: "Pager messages are private communications in many countries.",
      url: "https://github.com/EliasOenal/multimon-ng",
    },
  },
};

export const RTL433_RAW = {
  plugin: {
    id: RTL433_ID,
    name: "ISM sensors (rtl_433)",
    version: "0.1.0",
    protocol: 1,
    license: "AGPL-3.0-only",
    type: "decoder",
    description: "Decodes weather stations and other ISM-band devices",
  },
  runtime: { kind: "process", transport: "simple", command: ["superhet-plugin-adapter", "rtl433"] },
  input: { kind: "iq", sample_rate: 250000, format: "cu8", bandwidth_hz: 250000 },
  device: { exclusive: false },
  install: { acknowledge: { text: "Neighbours' sensors send identifiers." } },
};

export const ADSB_RAW = {
  plugin: {
    id: ADSB_ID,
    name: "ADS-B",
    version: "1.0.0",
    protocol: 1,
    license: "GPL-3.0-only",
    type: "decoder",
    description: "Aircraft transponders at 1090 MHz",
  },
  runtime: { kind: "process", transport: "simple", command: ["bin/adsb"] },
  input: { kind: "iq", sample_rate: 2400000, format: "cu8" },
  device: { exclusive: true, sample_rate: 2400000, center_hz: 1090000000 },
};

export const APRS_RAW = {
  plugin: {
    id: APRS_ID,
    name: "APRS (1200 baud)",
    version: "0.1.0",
    protocol: 1,
    license: "AGPL-3.0-only",
    type: "decoder",
    description: "Decodes APRS packets with direwolf",
  },
  runtime: { kind: "process", transport: "simple", command: ["superhet-plugin-adapter", "aprs"] },
  input: { kind: "discriminator", sample_rate: 22050, format: "s16le", bandwidth_hz: 12500 },
  device: { exclusive: false },
  install: { acknowledge: { text: "APRS positions can be personal data." } },
};

export const aprs: PluginManifest = parseManifest(APRS_RAW);
export const pocsag: PluginManifest = parseManifest(POCSAG_RAW);
export const rtl433: PluginManifest = parseManifest(RTL433_RAW);
export const adsb: PluginManifest = parseManifest(ADSB_RAW);

export const instanceRaw = (over: Record<string, unknown> = {}) => ({
  id: "i-1",
  plugin_id: POCSAG_ID,
  device: "dev0",
  vfo: 1,
  freq_hz: 439987500,
  config: null,
  exclusive: false,
  state: "running",
  pid: 4242,
  restarts: 0,
  crashes_in_window: 0,
  retry_in_ms: null,
  last_error: null,
  events_ok: 3,
  events_invalid: 0,
  input_dropped: 0,
  input_error: null,
  updated_unix_ms: 1_700_000_000_000,
  input_bytes: 441_000,
  input_level_db: -32.5,
  last_input_unix_ms: 1_700_000_000_000,
  last_event_unix_ms: 1_700_000_000_000,
  created_unix_ms: 1_699_999_000_000,
  reconfigures: 0,
  ...over,
});

export const instance = (over: Record<string, unknown> = {}): PluginInstance =>
  parseInstance(instanceRaw(over));

/** A fake plugin API whose lists the tests can change between calls. */
export function fakePluginApi(
  plugins: PluginManifest[] = [pocsag, rtl433],
  instances: PluginInstance[] = [],
) {
  const state = { plugins, instances };
  const api = {
    listPlugins: vi.fn(() => Promise.resolve(state.plugins)),
    install: vi.fn(() => Promise.resolve(pocsag)),
    remove: vi.fn(() => Promise.resolve()),
    listInstances: vi.fn(() => Promise.resolve(state.instances)),
    start: vi.fn((req: { pluginId: string; vfo?: number; device?: string }) => {
      const created = instance({
        id: `i-${state.instances.length + 1}`,
        plugin_id: req.pluginId,
        device: req.device ?? null,
        vfo: req.vfo ?? null,
      });
      state.instances = [...state.instances, created];
      return Promise.resolve(created);
    }),
    stop: vi.fn((id: string) => {
      const found = state.instances.find((i) => i.id === id) ?? instance({ id });
      state.instances = state.instances.filter((i) => i.id !== id);
      return Promise.resolve({ ...found, state: "stopped" as const });
    }),
    restart: vi.fn((id: string) =>
      Promise.resolve(state.instances.find((i) => i.id === id) ?? instance({ id })),
    ),
    reconfigure: vi.fn((id: string, config: Rec) => {
      const found = state.instances.find((i) => i.id === id) ?? instance({ id });
      const next = { ...found, config, reconfigures: found.reconfigures + 1 };
      state.instances = state.instances.map((i) => (i.id === id ? next : i));
      return Promise.resolve(next);
    }),
  } satisfies PluginApi;
  return { api, state };
}
