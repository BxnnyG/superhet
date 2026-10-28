// Fixtures for the DAB view tests: the plugin's status document and reply as the API sends them,
// and fake APIs.

import { vi } from "vitest";
import type { EventApi } from "../decoders/api";
import type { DabApi } from "./api";
import { parseReply, type DabReply } from "./model";

export function dabStatusRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    mode: "tuned",
    channel: "5C",
    freq_hz: 178352000,
    receiver_running: true,
    synced: true,
    snr: 14.5,
    fic_crc_errors: 2,
    iq_connected: true,
    ensemble: { label: "DR Deutschland", eid: "D210", ecc: "E0" },
    services: [
      {
        sid: "D210",
        label: "Deutschlandfunk",
        pty: "Information",
        language: "German",
        codec: "DAB+",
        bitrate: 72,
        protection: "EEP 3-A",
        audio: true,
        dls: "Nachrichten",
        audio_errors: 1,
      },
      {
        sid: "D212",
        label: "DKultur",
        pty: "Culture",
        language: "German",
        codec: "DAB",
        bitrate: 128,
        protection: "UEP 3",
        audio: true,
        dls: null,
        audio_errors: null,
      },
    ],
    selected: "D210",
    playing: true,
    scan: null,
    last_scan: null,
    error: null,
    ...over,
  };
}

export function dabReplyRaw(over: Record<string, unknown> = {}) {
  return {
    v: 1,
    ok: true,
    plugin_id: "org.superhet.decoder.dab",
    device: "dev0",
    state: "running",
    freq_hz: 178352000,
    audio_vfo: 1073741900,
    status: dabStatusRaw(),
    updated_unix_ms: 1790000000000,
    media: [{ key: "slide.d210", mime: "image/png", size: 8, revision: 2 }],
    ...over,
  };
}

export const dabReply = (over: Record<string, unknown> = {}): DabReply =>
  parseReply(dabReplyRaw(over));

export function fakeDabApi(initial: DabReply = dabReply()) {
  const state = { reply: initial };
  const api = {
    status: vi.fn(() => Promise.resolve(state.reply)),
    control: vi.fn(() => Promise.resolve()),
  } satisfies DabApi;
  return { api, state };
}

export function fakeEventApi(events: Awaited<ReturnType<EventApi["list"]>>["events"] = []) {
  return {
    list: vi.fn(() => Promise.resolve({ events, next: null })),
    clear: vi.fn(() => Promise.resolve(0)),
  } satisfies EventApi;
}
