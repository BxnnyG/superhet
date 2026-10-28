// Is a decoder getting input and decoding? Pure helpers plus the hook that tells whether the
// input counter of an instance keeps growing. The host reports counters, not rates, and the browser
// clock may differ from the server's, so "flowing" is judged by the byte counter growing between
// two polls, and times are shown as clock times, never as "x s ago" computed across machines.

import { useState } from "react";
import { formatTime } from "../decoders/events";
import type { BadgeTone } from "../ui";
import type { PluginInstance } from "./api";

export type Flow = "unknown" | "flowing" | "stalled";

interface Track {
  id: string;
  bytes: number;
  snapshot: PluginInstance;
  flow: Flow;
}

/** Whether the byte counter of the instance grew since the previous snapshot. */
export function useFlow(instance: PluginInstance | undefined): Flow {
  const [track, setTrack] = useState<Track | null>(null);
  if (instance === undefined) {
    if (track !== null) setTrack(null);
    return "unknown";
  }
  if (track === null || track.id !== instance.id) {
    const flow: Flow = instance.inputBytes > 0 ? "flowing" : "unknown";
    setTrack({ id: instance.id, bytes: instance.inputBytes, snapshot: instance, flow });
    return flow;
  }
  // Only a fresh snapshot counts; re-renders with the same object say nothing.
  if (track.snapshot !== instance) {
    const flow: Flow = instance.inputBytes > track.bytes ? "flowing" : "stalled";
    setTrack({ id: instance.id, bytes: instance.inputBytes, snapshot: instance, flow });
    return flow;
  }
  return track.flow;
}

export function formatLevel(db: number | null): string {
  if (db === null) return "no signal data";
  return `${db <= -120 ? "<-120" : db.toFixed(0)} dBFS`;
}

/** "1.2 MB", "48 kB". */
export function formatBytes(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
  if (n >= 1e3) return `${Math.round(n / 1e3)} kB`;
  return `${n} B`;
}

export interface Verdict {
  tone: BadgeTone;
  /** One line that says what is going on with the input. */
  text: string;
}

/**
 * The first thing to check when nothing is decoded: is the decoder running, does it get samples,
 * did it decode anything.
 */
export function verdict(i: PluginInstance, flow: Flow): Verdict {
  switch (i.state) {
    case "starting":
      return { tone: "warn", text: "Starting the decoder…" };
    case "backoff":
      return { tone: "warn", text: "The decoder ended unexpectedly and restarts shortly." };
    case "error":
      return { tone: "danger", text: "The decoder failed repeatedly and was given up." };
    case "stopped":
      return { tone: "neutral", text: "Stopped." };
    case "running":
      break;
  }
  if (i.inputError !== null) return { tone: "danger", text: `No input: ${i.inputError}` };
  if (i.vfo !== null && i.inputBytes === 0) {
    return {
      tone: "warn",
      text: "No samples have reached the decoder yet. Check that the receiver is active (not paused outside the window).",
    };
  }
  if (i.vfo !== null && flow === "stalled") {
    return {
      tone: "warn",
      text: "The input stopped: no new samples reached the decoder. The receiver may be paused or the device busy.",
    };
  }
  if (i.eventsOk === 0) {
    return { tone: "success", text: "Listening: samples arrive, nothing decoded yet." };
  }
  return { tone: "success", text: "Decoding." };
}

/** Clock time of the last decoded event, or null. */
export function lastDecodeText(i: PluginInstance): string | null {
  return i.lastEventMs === null ? null : formatTime(i.lastEventMs);
}
