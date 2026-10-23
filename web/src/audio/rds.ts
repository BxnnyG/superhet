// RDS state of a VFO as sent by the daemon on the audio socket (docs/protocol-audio.md,
// "RDS state"). One message is the complete state; an empty one clears the display.

export interface RdsState {
  /** Block synchronisation holds right now, i.e. data is being received. */
  synced: boolean;
  /** Programme identification, four upper-case hex digits. */
  pi: string | null;
  /** Programme type 0..31. */
  pty: number | null;
  /** Traffic programme / traffic announcement flags. */
  tp: boolean | null;
  ta: boolean | null;
  /** Music/speech switch: true = music. */
  ms: boolean | null;
  /** Programme service name (8 characters). */
  ps: string | null;
  /** RadioText. */
  rt: string | null;
}

export interface RdsMessage {
  vfo: number;
  state: RdsState;
}

const str = (v: unknown): string | null => (typeof v === "string" ? v : null);
const bool = (v: unknown): boolean | null => (typeof v === "boolean" ? v : null);

/** Validate a decoded JSON message; null if it is not an RDS message. */
export function parseRds(msg: unknown): RdsMessage | null {
  if (typeof msg !== "object" || msg === null) return null;
  const m = msg as Record<string, unknown>;
  if (m.type !== "rds" || m.v !== 1) return null;
  const vfo = m.vfo;
  if (typeof vfo !== "number" || !Number.isInteger(vfo) || vfo < 1) return null;
  const pty = typeof m.pty === "number" && m.pty >= 0 && m.pty <= 31 ? m.pty : null;
  const pi = typeof m.pi === "string" && /^[0-9A-Fa-f]{4}$/.test(m.pi) ? m.pi.toUpperCase() : null;
  return {
    vfo,
    state: {
      synced: m.synced === true,
      pi,
      pty,
      tp: bool(m.tp),
      ta: bool(m.ta),
      ms: bool(m.ms),
      ps: str(m.ps),
      rt: str(m.rt),
    },
  };
}

export function isEmptyRds(s: RdsState): boolean {
  return (
    !s.synced &&
    s.pi === null &&
    s.pty === null &&
    s.tp === null &&
    s.ta === null &&
    s.ms === null &&
    s.ps === null &&
    s.rt === null
  );
}

export function sameRds(a: RdsState, b: RdsState): boolean {
  return (
    a.synced === b.synced &&
    a.pi === b.pi &&
    a.pty === b.pty &&
    a.tp === b.tp &&
    a.ta === b.ta &&
    a.ms === b.ms &&
    a.ps === b.ps &&
    a.rt === b.rt
  );
}

/** Programme types of IEC 62106 (Europe). */
const PTY_RDS = [
  "None",
  "News",
  "Current affairs",
  "Information",
  "Sport",
  "Education",
  "Drama",
  "Culture",
  "Science",
  "Varied",
  "Pop music",
  "Rock music",
  "Easy listening",
  "Light classical",
  "Serious classical",
  "Other music",
  "Weather",
  "Finance",
  "Children's programmes",
  "Social affairs",
  "Religion",
  "Phone-in",
  "Travel",
  "Leisure",
  "Jazz music",
  "Country music",
  "National music",
  "Oldies music",
  "Folk music",
  "Documentary",
  "Alarm test",
  "Alarm",
];

/** Programme types of the North American RBDS variant. */
const PTY_RBDS = [
  "None",
  "News",
  "Information",
  "Sports",
  "Talk",
  "Rock",
  "Classic rock",
  "Adult hits",
  "Soft rock",
  "Top 40",
  "Country",
  "Oldies",
  "Soft",
  "Nostalgia",
  "Jazz",
  "Classical",
  "Rhythm and blues",
  "Soft R&B",
  "Foreign language",
  "Religious music",
  "Religious talk",
  "Personality",
  "Public",
  "College",
  "Spanish talk",
  "Spanish music",
  "Hip hop",
  "Unassigned",
  "Unassigned",
  "Weather",
  "Emergency test",
  "Emergency",
];

/**
 * Name of a programme type. The code is the same worldwide but its meaning differs between
 * Europe (50 µs de-emphasis) and North America (75 µs, RBDS), so the de-emphasis picks the table.
 */
export function ptyName(pty: number, deemphasis: string | null): string {
  const table = deemphasis === "75us" ? PTY_RBDS : PTY_RDS;
  return table[pty] ?? `PTY ${pty}`;
}
