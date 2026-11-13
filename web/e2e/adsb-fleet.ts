// A mixed fleet around a receiver at 51.0 / 7.0 for the ADS-B map scenario: one aircraft of each
// symbol kind, served through the status and lookup endpoints so the map has something of every
// type to draw. Positions drift a little with every poll so the trails grow.

export interface FleetEntry {
  icao: string;
  callsign: string | null;
  /** Expected symbol kind (web/src/adsb/symbols.ts). */
  kind: string;
  lat: number;
  lon: number;
  track: number;
  alt_ft?: number;
  on_ground?: true;
  speed_kt: number;
  category?: string;
  squawk?: string;
  /** What the aircraft database knows (type designator, ICAO class, wake, military). */
  db?: {
    reg: string;
    type: string;
    type_desc: string;
    class: string;
    wake: string;
    military?: boolean;
  };
}

export const FLEET: FleetEntry[] = [
  {
    icao: "3c64a1",
    callsign: "DLH400",
    kind: "wide",
    lat: 51.25,
    lon: 7.35,
    track: 250,
    alt_ft: 38000,
    speed_kt: 490,
    category: "A5",
    db: { reg: "D-AIXA", type: "A359", type_desc: "AIRBUS A-350-900", class: "L2J", wake: "H" },
  },
  {
    icao: "4ca7c3",
    callsign: "RYR8QK",
    kind: "narrow",
    lat: 50.8,
    lon: 7.6,
    track: 80,
    alt_ft: 31000,
    speed_kt: 440,
    category: "A3",
    db: { reg: "EI-DWK", type: "B738", type_desc: "BOEING 737-800", class: "L2J", wake: "M" },
  },
  {
    icao: "406a3d",
    callsign: "BAW117",
    kind: "quad",
    lat: 51.6,
    lon: 6.4,
    track: 110,
    alt_ft: 36000,
    speed_kt: 520,
    category: "A5",
    db: { reg: "G-CIVA", type: "B748", type_desc: "BOEING 747-8", class: "L4J", wake: "H" },
  },
  {
    icao: "a1b2c3",
    callsign: "N650GD",
    kind: "bizjet",
    lat: 50.5,
    lon: 6.5,
    track: 20,
    alt_ft: 41000,
    speed_kt: 470,
    category: "A2",
    db: { reg: "N650GD", type: "GLF6", type_desc: "GULFSTREAM G650", class: "L2J", wake: "M" },
  },
  {
    icao: "3e1a01",
    callsign: "DEABC",
    kind: "light",
    lat: 51.05,
    lon: 7.1,
    track: 320,
    alt_ft: 3500,
    speed_kt: 105,
    category: "A1",
    db: { reg: "D-EABC", type: "C172", type_desc: "CESSNA 172", class: "L1P", wake: "L" },
  },
  {
    icao: "3e1a02",
    callsign: "DFGHI",
    kind: "twinprop",
    lat: 51.15,
    lon: 6.8,
    track: 160,
    alt_ft: 12000,
    speed_kt: 250,
    db: {
      reg: "D-FGHI",
      type: "DH8D",
      type_desc: "DE HAVILLAND DASH 8-400",
      class: "L2T",
      wake: "M",
    },
  },
  {
    icao: "3e1a03",
    callsign: "CHX33",
    kind: "heli",
    lat: 50.95,
    lon: 7.0,
    track: 45,
    alt_ft: 1200,
    speed_kt: 110,
    category: "A7",
    db: { reg: "D-HXYZ", type: "EC35", type_desc: "EUROCOPTER EC-135", class: "H2T", wake: "L" },
  },
  {
    icao: "3e1a04",
    callsign: "GLIDER",
    kind: "glider",
    lat: 50.7,
    lon: 6.9,
    track: 200,
    alt_ft: 5500,
    speed_kt: 55,
    category: "B1",
  },
  {
    icao: "3e1a05",
    callsign: "BALLON",
    kind: "balloon",
    lat: 51.35,
    lon: 7.7,
    track: 90,
    alt_ft: 2500,
    speed_kt: 8,
    category: "B2",
  },
  {
    icao: "3e1a06",
    callsign: "DRONE1",
    kind: "uav",
    lat: 51.02,
    lon: 7.25,
    track: 10,
    alt_ft: 400,
    speed_kt: 30,
    category: "B6",
  },
  {
    icao: "3f0001",
    callsign: "GAF123",
    kind: "fighter",
    lat: 51.4,
    lon: 6.7,
    track: 300,
    alt_ft: 24000,
    speed_kt: 480,
    db: {
      reg: "31+17",
      type: "EUFI",
      type_desc: "EUROFIGHTER TYPHOON",
      class: "L2J",
      wake: "M",
      military: true,
    },
  },
  {
    icao: "3e1a07",
    callsign: "VEH1",
    kind: "ground",
    lat: 50.88,
    lon: 7.15,
    track: 270,
    on_ground: true,
    speed_kt: 6,
    category: "C2",
  },
  {
    icao: "3e1a08",
    callsign: null,
    kind: "tower",
    lat: 50.98,
    lon: 6.7,
    track: 0,
    on_ground: true,
    speed_kt: 0,
    category: "C3",
  },
  {
    icao: "3e1a09",
    callsign: null,
    kind: "unknown",
    lat: 51.2,
    lon: 6.95,
    track: 135,
    alt_ft: 9000,
    speed_kt: 200,
  },
  {
    icao: "3e1a0a",
    callsign: "EMG777",
    kind: "narrow",
    lat: 50.6,
    lon: 7.4,
    track: 340,
    alt_ft: 14000,
    speed_kt: 300,
    category: "A3",
    squawk: "7700",
  },
];

/** The status reply for poll number `n`: everything has moved a little along its track. */
export function fleetReply(n: number) {
  const aircraft = FLEET.map((f) => {
    const step = f.on_ground ? 0 : 0.0006 * n * Math.min(1, f.speed_kt / 250 + 0.2);
    const rad = (f.track * Math.PI) / 180;
    return {
      icao: f.icao,
      ...(f.callsign === null ? {} : { callsign: f.callsign }),
      ...(f.alt_ft === undefined ? {} : { alt_ft: f.alt_ft }),
      ...(f.on_ground ? { on_ground: true } : {}),
      speed_kt: f.speed_kt,
      track_deg: f.track,
      vrate_fpm: f.alt_ft !== undefined && f.alt_ft > 20000 ? 0 : -500,
      alt_geom_ft: f.alt_ft === undefined ? undefined : f.alt_ft + 350,
      ias_kt:
        f.alt_ft !== undefined && f.speed_kt > 150 ? Math.round(f.speed_kt * 0.62) : undefined,
      mach: f.alt_ft !== undefined && f.alt_ft > 30000 ? 0.8 : undefined,
      lat: Number((f.lat + Math.cos(rad) * step).toFixed(5)),
      lon: Number((f.lon + Math.sin(rad) * step * 1.5).toFixed(5)),
      rssi: -12,
      ...(f.squawk === undefined ? {} : { squawk: f.squawk }),
      ...(f.category === undefined ? {} : { category: f.category }),
      messages: 100 + n,
      seen_s: 0.3,
      seen_pos_s: 0.3,
    };
  });
  return {
    v: 1,
    ok: true,
    plugin_id: "org.superhet.decoder.adsb",
    device: "dev0",
    state: "running",
    freq_hz: 1090000000,
    audio_vfo: null,
    media: [],
    updated_unix_ms: Date.now(),
    status: {
      v: 1,
      mode: "running",
      updated_ms: Date.now(),
      stale_after_s: 60,
      receiver: { lat: 51.0, lon: 7.0 },
      messages: 5000 + n,
      messages_per_s: 410,
      total: aircraft.length,
      with_position: aircraft.length,
      truncated: false,
      error: null,
      aircraft,
    },
  };
}

/** `GET /api/v1/adsb/lookup`: what the aircraft database knows about the fleet. */
export function fleetLookup() {
  const aircraft: Record<string, unknown> = {};
  for (const f of FLEET) {
    if (f.db) aircraft[f.icao] = { ...f.db, military: f.db.military === true };
  }
  return { v: 1, aircraft, operators: {} };
}
