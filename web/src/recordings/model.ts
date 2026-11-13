// Display helpers of the recordings UI.

import { formatMhz } from "../format";
import type { Recording } from "./api";

/** `1:05`, `12:03`, `1:02:03` for a duration in milliseconds. */
export function formatClock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const two = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${two(m)}:${two(sec)}` : `${m}:${two(sec)}`;
}

/** `512 B`, `3.4 MB`, `1.20 GB`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${units[i]}`;
}

export const isRunning = (r: Recording) => r.state === "recording";

/** The recording a record button should show as running for this VFO, if any. */
export function runningFor(items: readonly Recording[], vfoId: number): Recording | null {
  return items.find((r) => isRunning(r) && r.vfo === vfoId) ?? null;
}

const SOURCE_LABEL = { manual: "", replay: "Replay", scanner: "Scanner hit" } as const;

/** Short context line of a row: frequency, mode, source. */
export function subtitle(r: Recording): string {
  const parts: string[] = [];
  if (r.freqHz !== null) parts.push(formatMhz(r.freqHz));
  if (r.mode !== null) parts.push(r.mode.toUpperCase());
  if (r.kind === "iq") parts.push("IQ");
  const source = SOURCE_LABEL[r.source];
  if (source !== "") parts.push(source);
  parts.push(r.device);
  return parts.join(" · ");
}

export const STATE_TEXT: Record<Recording["state"], string> = {
  recording: "Recording",
  done: "Done",
  interrupted: "Interrupted",
  failed: "Failed",
};

/** The replay lengths offered, limited to what the server buffers. */
export function replayChoices(bufferSeconds: number): number[] {
  return [10, 30, 60].filter((s) => s <= bufferSeconds);
}

/** Why a recording ended, in plain words (from `meta.end_reason`). */
export function endReason(r: Recording): string | null {
  switch (r.meta.end_reason) {
    case "limit":
      return "Stopped at its size or time limit.";
    case "quota":
      return "Stopped: the recordings quota is used up.";
    case "stream_ended":
      return "Ended: the receiver stopped sending.";
    case "silence":
      return "Ended after the signal stopped.";
    case "error":
      return "Stopped by a storage error.";
    default:
      return null;
  }
}
