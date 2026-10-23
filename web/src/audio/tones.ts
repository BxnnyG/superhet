// Tone state of an NFM / AM VFO as sent by the daemon on the audio socket
// (docs/protocol-audio.md, "Tone state"): the CTCSS sub-tone currently present and the DTMF digits
// detected. A message is the complete state, not a delta; the digit string is accumulated here
// from the running count so lost or repeated messages cannot corrupt it.

/** What the UI shows for one VFO. */
export interface ToneState {
  /** CTCSS sub-tone currently present in Hz. */
  ctcssHz: number | null;
  /** DTMF digits shown, oldest first (cleared by the user, kept across messages otherwise). */
  digits: string;
  /** `dtmf_count` of the last message folded into `digits`. */
  count: number;
}

export interface TonesMessage {
  vfo: number;
  ctcssHz: number | null;
  dtmfCount: number;
  dtmfTail: string;
}

/** Longest digit string kept (older digits scroll out). */
export const MAX_DIGITS = 128;

const DIGIT_RE = /^[0-9A-D*#]*$/;

/** Validate a decoded JSON message; null if it is not a tones message. */
export function parseTones(msg: unknown): TonesMessage | null {
  if (typeof msg !== "object" || msg === null) return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "tones" || m.v !== 1) return null;
  const vfo = m.vfo;
  if (typeof vfo !== "number" || !Number.isInteger(vfo) || vfo < 1) return null;
  const count = m.dtmf_count;
  if (typeof count !== "number" || !Number.isInteger(count) || count < 0) return null;
  const tail = typeof m.dtmf_tail === "string" && DIGIT_RE.test(m.dtmf_tail) ? m.dtmf_tail : "";
  const hz = m.ctcss_hz;
  const ctcssHz = typeof hz === "number" && Number.isFinite(hz) && hz > 0 ? hz : null;
  return { vfo, ctcssHz, dtmfCount: count, dtmfTail: tail };
}

export function isEmptyTones(m: TonesMessage): boolean {
  return m.ctcssHz === null && m.dtmfCount === 0;
}

function cap(digits: string): string {
  return digits.length > MAX_DIGITS ? digits.slice(digits.length - MAX_DIGITS) : digits;
}

/**
 * Fold a message into the displayed state. New digits are the last `count - previous count`
 * characters of the tail; a counter that went backwards means the channel restarted.
 */
export function applyTones(prev: ToneState | undefined, msg: TonesMessage): ToneState {
  if (prev === undefined || msg.dtmfCount < prev.count) {
    return { ctcssHz: msg.ctcssHz, digits: cap(msg.dtmfTail), count: msg.dtmfCount };
  }
  const fresh = msg.dtmfCount - prev.count;
  const added = fresh === 0 ? "" : msg.dtmfTail.slice(Math.max(0, msg.dtmfTail.length - fresh));
  return { ctcssHz: msg.ctcssHz, digits: cap(prev.digits + added), count: msg.dtmfCount };
}

export function sameTones(a: ToneState, b: ToneState): boolean {
  return a.ctcssHz === b.ctcssHz && a.digits === b.digits && a.count === b.count;
}

/** The 50 standard CTCSS tones in Hz, ascending. */
export const CTCSS_TONES: readonly number[] = [
  67.0, 69.3, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2,
  110.9, 114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 159.8, 162.2, 165.5,
  167.9, 171.3, 173.8, 177.3, 179.9, 183.5, 186.2, 189.9, 192.8, 196.6, 199.5, 203.5, 206.5, 210.7,
  218.1, 225.7, 229.1, 233.6, 241.8, 250.3, 254.1,
];

export function formatCtcss(hz: number): string {
  return `${hz.toFixed(1)} Hz`;
}
