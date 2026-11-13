// Which symbol an aircraft gets on the map, and the symbols themselves.
//
// The kind comes from what is known, best source first: the ICAO type designator ("A320",
// "GLF6") and the ICAO type description ("L2J": land plane, two engines, jet; "H1T": helicopter)
// plus the wake category from the aircraft database, then the emitter category the aircraft
// sends itself ("A3", "B2", ...). Without any of it the symbol is a neutral arrow, not a guess.
//
// The silhouettes are original and deliberately simple: one half outline per kind, mirrored, on a
// 32 x 32 grid with the nose up (they are rotated to the track). They are not copies of any other
// map's artwork.

export const SYMBOL_KINDS = [
  "light",
  "twinprop",
  "bizjet",
  "narrow",
  "wide",
  "quad",
  "heli",
  "glider",
  "balloon",
  "uav",
  "fighter",
  "ground",
  "tower",
  "unknown",
] as const;
export type SymbolKind = (typeof SYMBOL_KINDS)[number];

export const SYMBOL_LABEL: Record<SymbolKind, string> = {
  light: "Light propeller aircraft",
  twinprop: "Twin propeller or turboprop",
  bizjet: "Business or light jet",
  narrow: "Narrow-body airliner",
  wide: "Wide-body airliner",
  quad: "Four-engine aircraft",
  heli: "Helicopter",
  glider: "Glider or ultralight",
  balloon: "Balloon or airship",
  uav: "Drone (UAV)",
  fighter: "Military fast jet",
  ground: "Ground vehicle",
  tower: "Tower or obstacle",
  unknown: "Aircraft (type unknown)",
};

/** Symbol edge length in pixels: bigger aircraft get bigger symbols. */
export const SYMBOL_SIZE: Record<SymbolKind, number> = {
  light: 24,
  twinprop: 28,
  bizjet: 26,
  narrow: 30,
  wide: 38,
  quad: 42,
  heli: 30,
  glider: 32,
  balloon: 24,
  uav: 22,
  fighter: 26,
  ground: 18,
  tower: 18,
  unknown: 22,
};

/** Kinds without a heading worth showing: the symbol stays upright. */
export const UPRIGHT: ReadonlySet<SymbolKind> = new Set(["balloon", "tower"]);

/** What the type filter offers (several kinds per entry). */
export const KIND_GROUPS = [
  { id: "airliner", label: "Airliners", kinds: ["narrow", "wide", "quad"] },
  { id: "business", label: "Business jets", kinds: ["bizjet"] },
  { id: "light", label: "Light and turboprop", kinds: ["light", "twinprop"] },
  { id: "rotor", label: "Helicopters", kinds: ["heli"] },
  { id: "fighter", label: "Military fast jets", kinds: ["fighter"] },
  {
    id: "other",
    label: "Other",
    kinds: ["glider", "balloon", "uav", "ground", "tower", "unknown"],
  },
] as const satisfies readonly { id: string; label: string; kinds: readonly SymbolKind[] }[];
export type KindGroupId = (typeof KIND_GROUPS)[number]["id"];

export function groupOf(kind: SymbolKind): KindGroupId {
  return KIND_GROUPS.find((g) => (g.kinds as readonly string[]).includes(kind))?.id ?? "other";
}

/** What the classification looks at; every field is optional. */
export interface SymbolInput {
  /** ICAO type designator, e.g. "A359". */
  typeCode?: string | null;
  /** ICAO type description, e.g. "L2J". */
  typeClass?: string | null;
  /** Wake category: L, M, H or J. */
  wake?: string | null;
  /** ADS-B emitter category, e.g. "A3". */
  category?: string | null;
  military?: boolean;
}

const QUAD =
  /^(A124|A225|A34[2-6]|A388|A400|B52|B70[3-9]|B720|B74.|C17|C5M|C130|C30J|DC8|IL76|IL96|AN12|L100|E3TF|E3CF|K35R)$/;
const WIDE =
  /^(A3(3\d|5[0-9K]|10|06|0B|ST)|B(77[0-9WLX]|78[0-9X]|76[2-4])|MD11|DC10|L101|IL86|A337)$/;
const BIZJET =
  /^(C25[A-Z]?|C5[0-9][A-Z0-9]?|C6[0-9][A-Z0-9]?|C7[0-9][A-Z0-9]?|CL[3-6][0-9A-Z]|GLF[0-9A-Z]|GL[5-7]T|G[1-6][0-9]{2}|GALX|GLEX|FA[0-9][0-9A-Z]|F2TH|F900|LJ[0-9]+|E5[05]P|E545|E550|H25[A-C]|HDJT|PRM1|ASTR|WW24|BE40|SF50|EA50|LEGA|PC24)$/;
const FIGHTER =
  /^(F(1[0-9]|22|35|4|5|104|117)[A-Z]?|FA18|EUFI|RFAL|TORN|GRIN|GRIP|M2K|MIR[2F]|HAWK|A10|AV8B?|JAGR|SU[0-9]+|MIG[0-9]*|J10|J20|T38|HUNT|TYPH|B1|B2|L39|L159)$/;
const BALLOON = /^(BALL|HAIR|ZEPP|SHIP|AIRS)$/;
const GLIDER = /^(GLID|GLDR|SAIL)$/;
const UAV = /^(UAV|DRON|MQ[0-9]+|RQ[0-9]+)$/;
const GROUND = /^(GRND|SERV|GND|VEH)$/;
const TOWER = /^(TWR|TOWR|OBST)$/;

/** Kind from the type designator alone (the quick, exact cases), or null. */
function byDesignator(code: string): SymbolKind | null {
  if (BALLOON.test(code)) return "balloon";
  if (GLIDER.test(code)) return "glider";
  if (UAV.test(code)) return "uav";
  if (GROUND.test(code)) return "ground";
  if (TOWER.test(code)) return "tower";
  if (FIGHTER.test(code)) return "fighter";
  if (QUAD.test(code)) return "quad";
  if (WIDE.test(code)) return "wide";
  if (BIZJET.test(code)) return "bizjet";
  return null;
}

/** Kind from the ICAO type description (Doc 8643: aircraft class, engine count, engine type). */
function byClass(cls: string, wake: string, military: boolean): SymbolKind | null {
  if (/^[HGT]/.test(cls)) return "heli";
  const parsed = /^[LSA](\d)([PTJER])$/.exec(cls);
  if (parsed === null) return null;
  const engines = Number(parsed[1]);
  const kind = parsed[2];
  if (kind === "R") return "fighter";
  if (kind === "J") {
    if (engines >= 4) return "quad";
    if (engines === 1) return military ? "fighter" : "bizjet";
    if (wake === "H" || wake === "J") return "wide";
    if (wake === "L") return "bizjet";
    return "narrow";
  }
  // Piston, turboprop, electric.
  if (engines >= 4) return "quad";
  if (engines >= 2) return "twinprop";
  return "light";
}

/** Kind from the emitter category the aircraft sends (DO-260B, set A, B and C). */
function byCategory(category: string): SymbolKind | null {
  switch (category) {
    case "A1":
      return "light";
    case "A2":
      return "bizjet";
    case "A3":
    case "A4":
      return "narrow";
    case "A5":
      return "wide";
    case "A6":
      return "fighter";
    case "A7":
      return "heli";
    case "B1":
    case "B4":
      return "glider";
    case "B2":
      return "balloon";
    case "B6":
      return "uav";
    case "C1":
    case "C2":
      return "ground";
    case "C3":
    case "C4":
    case "C5":
      return "tower";
    default:
      return null;
  }
}

export function symbolKind(input: SymbolInput): SymbolKind {
  const code = (input.typeCode ?? "").toUpperCase();
  const cls = (input.typeClass ?? "").toUpperCase();
  const wake = (input.wake ?? "").toUpperCase();
  const military = input.military === true;
  const category = (input.category ?? "").toUpperCase();
  // Not an aircraft at all: the type says so, whatever else is known.
  const exact = code === "" ? null : byDesignator(code);
  if (exact !== null) return exact;
  const described = cls === "" ? null : byClass(cls, wake, military);
  if (described !== null) return described;
  const emitted = byCategory(category);
  if (emitted !== null) return emitted;
  // Only the wake category is known.
  if (wake === "H" || wake === "J") return "wide";
  if (wake === "M") return "narrow";
  return "unknown";
}

// --- the silhouettes ------------------------------------------------------------------------

type Half = readonly (readonly [number, number])[];

/** A symmetric outline from its right half (nose to tail, offsets from the centre line x = 16). */
function mirrored(half: Half): string {
  const right = half.map(([dx, y]) => `${(16 + dx).toFixed(1)} ${y}`);
  const left = [...half].reverse().map(([dx, y]) => `${(16 - dx).toFixed(1)} ${y}`);
  return `M${[...right, ...left].join("L")}Z`;
}

/** Two ellipses (engines or rotors) placed symmetrically at x = 16 +- dx. */
const pair = (dx: number, y: number, rx: number, ry: number): string =>
  `<ellipse cx="${16 - dx}" cy="${y}" rx="${rx}" ry="${ry}"/><ellipse cx="${16 + dx}" cy="${y}" rx="${rx}" ry="${ry}"/>`;

const NARROW: Half = [
  [0, 1.5],
  [1.4, 4.5],
  [1.5, 12],
  [12.5, 18],
  [12.5, 20],
  [1.5, 19.4],
  [1.3, 26],
  [5.2, 28.4],
  [5.2, 30],
  [1, 29.2],
  [0, 30.5],
];
const WIDE_HALF: Half = [
  [0, 1],
  [1.9, 4.5],
  [2, 11],
  [14.5, 18.5],
  [14.5, 21],
  [2, 20.6],
  [1.7, 26.5],
  [6, 29],
  [6, 30.8],
  [1.2, 30],
  [0, 31.4],
];
const QUAD_HALF: Half = [
  [0, 0.8],
  [1.9, 4],
  [2, 11],
  [15.2, 17.6],
  [15.2, 20.4],
  [2, 20.8],
  [1.7, 26.5],
  [6.4, 28.8],
  [6.4, 30.6],
  [1.2, 30],
  [0, 31.4],
];
const BIZ: Half = [
  [0, 3],
  [1.2, 6],
  [1.3, 13],
  [9, 18.2],
  [9, 20],
  [1.3, 19.4],
  [1.3, 25],
  [4, 27],
  [4, 28.4],
  [0.9, 28],
  [0, 29.5],
];
const LIGHT: Half = [
  [0, 4.5],
  [1.2, 7],
  [1.3, 10],
  [11.5, 10.4],
  [11.5, 14.4],
  [1.3, 14.6],
  [1.1, 24],
  [5.3, 25.2],
  [5.3, 27.4],
  [1, 27.6],
  [0, 28.6],
];
const TWIN: Half = [
  [0, 2.5],
  [1.4, 5.5],
  [1.5, 11],
  [13.5, 11.8],
  [13.5, 15.6],
  [1.5, 15.8],
  [1.3, 25],
  [5.6, 26.4],
  [5.6, 28.6],
  [1, 28.8],
  [0, 30],
];
const GLIDE: Half = [
  [0, 3],
  [0.9, 6],
  [1, 13],
  [15.4, 13.4],
  [15.4, 15.6],
  [1, 15.8],
  [0.8, 26],
  [4.2, 27],
  [4.2, 28.4],
  [0.6, 28.4],
  [0, 29.5],
];
const FIGHT: Half = [
  [0, 1],
  [1.1, 8],
  [1.4, 15],
  [9.8, 25],
  [9.8, 26.6],
  [2, 25.8],
  [1.7, 28.5],
  [4, 30.4],
  [0, 30],
];
const ARROW: Half = [
  [0, 4],
  [6, 25],
  [0, 21],
];

const PROP_LINE = (dx: number, y: number): string =>
  `<path d="M${16 - dx - 2.6} ${y}h5.2M${16 + dx - 2.6} ${y}h5.2" fill="none" stroke-width="1.4" stroke-linecap="round"/>`;

/** The inside of a 32 x 32 symbol: filled shapes use the current colour. */
const SHAPES: Record<SymbolKind, string> = {
  narrow: `<path d="${mirrored(NARROW)}"/>${pair(5.6, 15.2, 1.3, 2.2)}`,
  wide: `<path d="${mirrored(WIDE_HALF)}"/>${pair(6.2, 15.4, 1.5, 2.4)}`,
  quad: `<path d="${mirrored(QUAD_HALF)}"/>${pair(5.4, 14.6, 1.3, 2.2)}${pair(9.8, 16.6, 1.3, 2.2)}`,
  bizjet: `<path d="${mirrored(BIZ)}"/>${pair(2.8, 23, 1.2, 2.6)}`,
  light: `<path d="${mirrored(LIGHT)}"/>${PROP_LINE(0, 4.3)}`,
  twinprop: `<path d="${mirrored(TWIN)}"/>${pair(5.6, 9.6, 1.2, 2.6)}${PROP_LINE(5.6, 6.8)}`,
  glider: `<path d="${mirrored(GLIDE)}"/>`,
  fighter: `<path d="${mirrored(FIGHT)}"/>`,
  heli:
    `<circle cx="16" cy="13" r="12" fill="none" stroke-width="1.1" stroke-dasharray="3 2.4"/>` +
    `<path d="M16 7.2c2.6 0 3.8 2.4 3.8 5.2s-1.2 5.6-3.8 5.6-3.8-2.8-3.8-5.6S13.4 7.2 16 7.2zM15.2 18h1.6l.4 10.8h-2.4z"/>` +
    `<path d="M12.6 28.6h6.8" fill="none" stroke-width="1.5" stroke-linecap="round"/>`,
  balloon:
    `<path d="M16 3c5.4 0 9 3.8 9 8.6 0 3.6-2.2 6.4-4.6 8.2h-8.8C9.2 18 7 15.2 7 11.6 7 6.8 10.6 3 16 3z"/>` +
    `<rect x="13" y="23" width="6" height="5" rx="1"/>`,
  uav:
    `<path d="M7 7l18 18M25 7L7 25" fill="none" stroke-width="2"/>` +
    `<circle cx="7" cy="7" r="3.2"/><circle cx="25" cy="7" r="3.2"/><circle cx="7" cy="25" r="3.2"/><circle cx="25" cy="25" r="3.2"/>` +
    `<rect x="12" y="12" width="8" height="8" rx="2"/>`,
  ground: `<rect x="10" y="7" width="12" height="18" rx="3"/><rect x="12.5" y="9.5" width="7" height="4" rx="1" class="adsb-cut"/>`,
  tower: `<path d="M16 3l9 25H7z"/><path d="M12 19h8M10.5 24h11" class="adsb-cut" fill="none" stroke-width="1.4"/>`,
  unknown: `<path d="${mirrored(ARROW)}"/>`,
};

/** A complete symbol element: pointing north, coloured by the CSS `color`. */
export function symbolSvg(kind: SymbolKind): string {
  const size = SYMBOL_SIZE[kind];
  return `<svg viewBox="0 0 32 32" width="${size}" height="${size}" aria-hidden="true" fill="currentColor" class="adsb-svg">${SHAPES[kind]}</svg>`;
}
