// Built-in band plan (Germany / IARU Region 1), curated in-repo. The BNetzA Frequenzplan is only
// published as PDF without an API, so there is no machine-readable source to import from; see
// docs/bandplan.md. Anything not verified against a primary source carries `approx: true`.
// Frequencies are written through mhz()/khz() so no float error creeps into channel math.

import type { VfoMode } from "../vfo/api";

export type CategoryId =
  | "broadcast"
  | "amateur"
  | "pmr446"
  | "freenet"
  | "lpd433"
  | "cb"
  | "srd"
  | "aviation"
  | "marine"
  | "weather"
  | "tetra"
  | "business"
  | "custom";

export interface Category {
  id: CategoryId;
  label: string;
  /** Bar/tick colour on the (always dark) spectrum canvas. */
  color: string;
  defaultOn: boolean;
}

export const CATEGORIES: readonly Category[] = [
  { id: "broadcast", label: "Broadcast (FM, DAB)", color: "#38bdf8", defaultOn: true },
  { id: "amateur", label: "Amateur radio", color: "#4ade80", defaultOn: true },
  { id: "pmr446", label: "PMR446", color: "#fb923c", defaultOn: true },
  { id: "freenet", label: "Freenet", color: "#f472b6", defaultOn: true },
  { id: "lpd433", label: "LPD433", color: "#facc15", defaultOn: true },
  { id: "cb", label: "CB radio", color: "#a78bfa", defaultOn: true },
  { id: "srd", label: "SRD / ISM", color: "#2dd4bf", defaultOn: true },
  { id: "aviation", label: "Aviation", color: "#f87171", defaultOn: true },
  { id: "marine", label: "Marine VHF / AIS", color: "#60a5fa", defaultOn: true },
  { id: "weather", label: "Weather satellites", color: "#c084fc", defaultOn: true },
  { id: "tetra", label: "BOS / TETRA", color: "#e11d48", defaultOn: true },
  { id: "business", label: "Business radio (approx.)", color: "#a1a1aa", defaultOn: false },
  { id: "custom", label: "Imported", color: "#e4e4e7", defaultOn: true },
];

export const CATEGORY_BY_ID: ReadonlyMap<CategoryId, Category> = new Map(
  CATEGORIES.map((c) => [c.id, c]),
);

export interface BandRange {
  id: string;
  category: CategoryId;
  name: string;
  /** Label for narrow bars when `name` does not fit. */
  short?: string;
  startHz: number;
  endHz: number;
  /** Modes/usage in words, e.g. "CW / SSB". Shown in the tooltip. */
  modes?: string;
  /** Id of the range this one is a sub-segment of; it must lie inside it. */
  parent?: string;
  note?: string;
  approx?: boolean;
  source: string;
}

interface PlanBase {
  id: string;
  category: CategoryId;
  name: string;
  /** Demodulator to tune with a click; omit when no VFO mode can decode it (click does nothing). */
  mode?: VfoMode;
  /** Modulation in words for the tooltip (defaults to the VFO mode). */
  modeLabel?: string;
  bandwidthHz?: number;
  note?: string;
  approx?: boolean;
  source: string;
}

/** `count` channels from `firstHz` in `stepHz` steps, numbered from `firstNumber`. */
export interface GridPlan extends PlanBase {
  kind: "grid";
  firstHz: number;
  stepHz: number;
  count: number;
  firstNumber?: number;
}

export interface ListChannel {
  label: string;
  freqHz: number;
  /** Full channel name; defaults to "<plan name> <label>". */
  name?: string;
  note?: string;
  /** Overrides the plan's mode/bandwidth for this channel. */
  mode?: VfoMode;
  bandwidthHz?: number;
}

export interface ListPlan extends PlanBase {
  kind: "list";
  channels: readonly ListChannel[];
}

export type ChannelPlan = GridPlan | ListPlan;

const mhz = (v: number) => Math.round(v * 1e6);
const khz = (v: number) => Math.round(v * 1e3);

const SRC_BNETZA = "BNetzA Frequenzplan / allgemeinzuteilungen";
const SRC_IARU = "IARU Region 1 band plan (VHF/UHF handbook), DARC";
const SRC_IARU_HF = "IARU Region 1 HF band plan, AFuV";
const SRC_ETSI_DAB = "ETSI EN 300 401 (DAB), Band III channel table";
const SRC_ITU = "ITU Radio Regulations";
const SRC_MARINE = "ITU-R M.1084 / Appendix 18 (marine VHF)";
const COARSE = "Coarse; assignments are per licence. Check the current BNetzA Frequenzplan.";

export const RANGES: readonly BandRange[] = [
  // Broadcast
  {
    id: "fm-broadcast",
    category: "broadcast",
    name: "FM broadcast",
    short: "FM",
    startHz: mhz(87.5),
    endHz: mhz(108),
    modes: "WFM",
    source: SRC_BNETZA,
  },
  {
    id: "dab-band3",
    category: "broadcast",
    name: "DAB+ Band III",
    short: "DAB",
    startHz: mhz(174),
    endHz: mhz(240),
    modes: "COFDM, 1.536 MHz blocks",
    source: SRC_ETSI_DAB,
  },

  // Amateur radio (IARU Region 1; German allocations where they are narrower, in the note)
  ...(
    [
      ["160m", "160 m", 1.81, 2.0, undefined],
      ["80m", "80 m", 3.5, 3.8, undefined],
      ["60m", "60 m", 5.3515, 5.3665, "Secondary, power limits apply."],
      ["40m", "40 m", 7.0, 7.2, undefined],
      ["30m", "30 m", 10.1, 10.15, "CW and digital modes only."],
      ["20m", "20 m", 14.0, 14.35, undefined],
      ["17m", "17 m", 18.068, 18.168, undefined],
      ["15m", "15 m", 21.0, 21.45, undefined],
      ["12m", "12 m", 24.89, 24.99, undefined],
      ["10m", "10 m", 28.0, 29.7, undefined],
    ] as const
  ).map(([id, name, start, end, note]): BandRange => ({
    id: `ham-${id}`,
    category: "amateur",
    name: `${name} amateur`,
    short: name,
    startHz: mhz(start),
    endHz: mhz(end),
    note,
    source: SRC_IARU_HF,
  })),
  {
    id: "ham-6m",
    category: "amateur",
    name: "6 m amateur",
    short: "6 m",
    startHz: mhz(50),
    endHz: mhz(52),
    note: "IARU R1 allocation; Germany allows only parts of it (class A: about 50.08-51.0 MHz).",
    approx: true,
    source: SRC_IARU,
  },
  {
    id: "ham-4m",
    category: "amateur",
    name: "4 m amateur",
    short: "4 m",
    startHz: mhz(70),
    endHz: mhz(70.5),
    note: "IARU R1 allocation; in Germany only a small segment is available. Check the AFuV.",
    approx: true,
    source: SRC_IARU,
  },
  {
    id: "ham-2m",
    category: "amateur",
    name: "2 m amateur",
    short: "2 m",
    startHz: mhz(144),
    endHz: mhz(146),
    source: SRC_IARU,
  },
  ...(
    [
      ["cw", "CW / EME", 144.0, 144.15, "CW; 144.050 CW calling."],
      ["ssb", "SSB / CW", 144.15, 144.4, "144.300 SSB calling."],
      ["beacons", "Beacons", 144.4, 144.5, undefined],
      ["mixed", "Narrowband / MGM", 144.5, 144.794, undefined],
      ["digital", "Digital / APRS", 144.794, 144.99, "144.800 APRS (Europe)."],
      ["rpt-in", "FM repeater input", 145.0, 145.194, undefined],
      ["fm-simplex", "FM simplex", 145.2, 145.594, "145.500 FM calling."],
      ["rpt-out", "FM repeater output", 145.594, 145.794, undefined],
      ["sat", "Satellite", 145.8, 146.0, "Amateur satellite service (e.g. ISS)."],
    ] as const
  ).map(([id, name, start, end, note]): BandRange => ({
    id: `ham-2m-${id}`,
    category: "amateur",
    name,
    startHz: mhz(start),
    endHz: mhz(end),
    parent: "ham-2m",
    note: [note, "Coarse sub-segment; follow the current IARU R1 / DARC band plan."]
      .filter(Boolean)
      .join(" "),
    approx: true,
    source: SRC_IARU,
  })),
  {
    id: "ham-70cm",
    category: "amateur",
    name: "70 cm amateur",
    short: "70 cm",
    startHz: mhz(430),
    endHz: mhz(440),
    note: "Shares 433.05-434.79 MHz with licence-free SRD/ISM users.",
    source: SRC_IARU,
  },
  {
    id: "ham-70cm-weak",
    category: "amateur",
    name: "Weak signal / EME",
    startHz: mhz(432),
    endHz: mhz(432.5),
    parent: "ham-70cm",
    note: "Coarse sub-segment; follow the current IARU R1 / DARC band plan.",
    approx: true,
    source: SRC_IARU,
  },
  {
    id: "ham-70cm-fm",
    category: "amateur",
    name: "FM simplex",
    startHz: mhz(433.4),
    endHz: mhz(433.575),
    parent: "ham-70cm",
    note: "433.500 FM calling. Coarse sub-segment; follow the current IARU R1 / DARC band plan.",
    approx: true,
    source: SRC_IARU,
  },
  {
    id: "ham-70cm-sat",
    category: "amateur",
    name: "Satellite",
    startHz: mhz(435),
    endHz: mhz(438),
    parent: "ham-70cm",
    note: "Amateur satellite service. Coarse sub-segment.",
    approx: true,
    source: SRC_IARU,
  },
  {
    id: "ham-23cm",
    category: "amateur",
    name: "23 cm amateur",
    short: "23 cm",
    startHz: mhz(1240),
    endHz: mhz(1300),
    source: SRC_IARU,
  },

  // Licence-free short range
  {
    id: "pmr446",
    category: "pmr446",
    name: "PMR446",
    startHz: mhz(446),
    endHz: mhz(446.2),
    modes: "NFM 12.5 kHz",
    note: "Licence-free handhelds, fixed antenna, 0.5 W.",
    source: SRC_BNETZA,
  },
  {
    id: "freenet",
    category: "freenet",
    name: "Freenet",
    startHz: mhz(149.01),
    endHz: mhz(149.125),
    modes: "NFM 12.5 kHz",
    note: "Licence-free, 6 channels, 0.5 W.",
    source: SRC_BNETZA,
  },
  {
    id: "lpd433",
    category: "lpd433",
    name: "LPD433",
    startHz: mhz(433.0625),
    endHz: mhz(434.7875),
    modes: "NFM 25 kHz raster",
    note: "Licence-free, 69 channels, 10 mW. Inside the 433 MHz SRD band.",
    source: SRC_BNETZA,
  },
  {
    id: "cb-80",
    category: "cb",
    name: "CB radio (80 ch)",
    short: "CB",
    startHz: mhz(26.56),
    endHz: mhz(27.41),
    modes: "AM / FM",
    note: "Channels 1-40 AM and FM, 41-80 (26.565-26.955 MHz) FM only; 4 W FM / 1 W AM.",
    source: SRC_BNETZA,
  },
  {
    id: "srd-433",
    category: "srd",
    name: "SRD / ISM 433",
    short: "ISM",
    startHz: mhz(433.05),
    endHz: mhz(434.79),
    modes: "OOK / FSK / NFM",
    note: "Remote controls, weather sensors (433.92 MHz centre), LPD433.",
    source: SRC_BNETZA,
  },
  {
    id: "srd-868",
    category: "srd",
    name: "SRD 863-870 MHz",
    short: "SRD",
    startHz: mhz(863),
    endHz: mhz(870),
    modes: "FSK / LoRa / others",
    note: "Sub-bands with duty-cycle limits (e.g. 868.0-868.6 MHz 1 %).",
    source: SRC_BNETZA,
  },

  // Aviation, marine, satellites
  {
    id: "air-nav",
    category: "aviation",
    name: "VOR / ILS navigation",
    short: "VOR/ILS",
    startHz: mhz(108),
    endHz: mhz(117.975),
    modes: "AM (nav signals)",
    source: SRC_ITU,
  },
  {
    id: "airband",
    category: "aviation",
    name: "Airband voice",
    short: "Airband",
    startHz: mhz(118),
    endHz: mhz(137),
    modes: "AM, 8.33 / 25 kHz",
    source: SRC_ITU,
  },
  {
    id: "air-mil",
    category: "aviation",
    name: "Military air UHF",
    short: "Mil air",
    startHz: mhz(225),
    endHz: mhz(400),
    modes: "AM",
    approx: true,
    source: SRC_ITU,
  },
  {
    id: "marine-vhf",
    category: "marine",
    name: "Marine VHF",
    short: "Marine",
    startHz: mhz(156),
    endHz: mhz(162.025),
    modes: "NFM 25 kHz",
    note: "Includes AIS 161.975 / 162.025 MHz.",
    source: SRC_MARINE,
  },
  {
    id: "wx-sat",
    category: "weather",
    name: "Weather satellites 137 MHz",
    short: "WX sat",
    startHz: mhz(137),
    endHz: mhz(138),
    modes: "APT (AM/FM) / LRPT (QPSK)",
    note: "Typical downlinks 137.100, 137.620, 137.9125 MHz; the active satellites and frequencies change over time.",
    approx: true,
    source: SRC_ITU,
  },

  // BOS / TETRA (listed so it is recognised, not to be received)
  ...(
    [
      ["uplink", 380, 385],
      ["downlink", 390, 395],
    ] as const
  ).map(([id, start, end]): BandRange => ({
    id: `bos-tetra-${id}`,
    category: "tetra",
    name: "BOS (TETRA) – reception not permitted, § 5 TDDDG",
    short: "BOS (TETRA)",
    startHz: mhz(start),
    endHz: mhz(end),
    modes: `TETRA ${id}`,
    note: "Authorities' digital radio. Listed for orientation only; reception of this traffic is not permitted.",
    source: SRC_BNETZA,
  })),

  // Business radio: coarse, approximate
  {
    id: "biz-4m",
    category: "business",
    name: "Business radio 4 m",
    short: "Biz 4 m",
    startHz: mhz(74.2),
    endHz: mhz(87.3),
    note: `Mixed use (also authorities); ${COARSE}`,
    approx: true,
    source: SRC_BNETZA,
  },
  {
    id: "biz-2m",
    category: "business",
    name: "Business radio 2 m",
    short: "Biz 2 m",
    startHz: mhz(146),
    endHz: mhz(174),
    note: `Overlaps Freenet and marine VHF; ${COARSE}`,
    approx: true,
    source: SRC_BNETZA,
  },
  {
    id: "biz-70cm",
    category: "business",
    name: "Business radio 70 cm",
    short: "Biz 70 cm",
    startHz: mhz(440),
    endHz: mhz(470),
    note: `Includes PMR446; ${COARSE}`,
    approx: true,
    source: SRC_BNETZA,
  },
];

/** The 40 standard CB channels in kHz (note the irregular order of 23, 24 and 25). */
const CB_STANDARD_KHZ = [
  26965, 26975, 26985, 27005, 27015, 27025, 27035, 27055, 27065, 27075, 27085, 27105, 27115, 27125,
  27135, 27155, 27165, 27175, 27185, 27205, 27215, 27225, 27255, 27235, 27245, 27265, 27275, 27285,
  27295, 27305, 27315, 27325, 27335, 27345, 27355, 27365, 27375, 27385, 27395, 27405,
] as const;

const DAB_BLOCKS: readonly (readonly [string, number])[] = [
  ["5A", 174.928],
  ["5B", 176.64],
  ["5C", 178.352],
  ["5D", 180.064],
  ["6A", 181.936],
  ["6B", 183.648],
  ["6C", 185.36],
  ["6D", 187.072],
  ["7A", 188.928],
  ["7B", 190.64],
  ["7C", 192.352],
  ["7D", 194.064],
  ["8A", 195.936],
  ["8B", 197.648],
  ["8C", 199.36],
  ["8D", 201.072],
  ["9A", 202.928],
  ["9B", 204.64],
  ["9C", 206.352],
  ["9D", 208.064],
  ["10A", 209.936],
  ["10B", 211.648],
  ["10C", 213.36],
  ["10D", 215.072],
  ["11A", 216.928],
  ["11B", 218.64],
  ["11C", 220.352],
  ["11D", 222.064],
  ["12A", 223.936],
  ["12B", 225.648],
  ["12C", 227.36],
  ["12D", 229.072],
  ["13A", 230.784],
  ["13B", 232.496],
  ["13C", 234.208],
  ["13D", 235.776],
  ["13E", 237.488],
  ["13F", 239.2],
];

export const CHANNEL_PLANS: readonly ChannelPlan[] = [
  {
    kind: "list",
    id: "dab",
    category: "broadcast",
    name: "DAB+ block",
    modeLabel: "COFDM, 1.536 MHz",
    bandwidthHz: khz(1536),
    note: "Digital radio ensemble; needs a DAB decoder.",
    source: SRC_ETSI_DAB,
    channels: DAB_BLOCKS.map(([label, f]) => ({ label, freqHz: mhz(f) })),
  },
  {
    kind: "list",
    id: "ham-calling",
    category: "amateur",
    name: "Amateur calling / APRS",
    source: SRC_IARU,
    channels: [
      { label: "SSB call", freqHz: khz(144300), name: "2 m SSB calling", note: "USB." },
      {
        label: "APRS",
        freqHz: khz(144800),
        name: "APRS (Europe)",
        note: "AX.25 1200 baud AFSK.",
        mode: "nfm",
        bandwidthHz: 12_500,
      },
      {
        label: "FM call",
        freqHz: khz(145500),
        name: "2 m FM calling",
        mode: "nfm",
        bandwidthHz: 12_500,
      },
      {
        label: "FM call",
        freqHz: khz(433500),
        name: "70 cm FM calling",
        mode: "nfm",
        bandwidthHz: 12_500,
      },
    ],
  },
  {
    kind: "grid",
    id: "pmr446",
    category: "pmr446",
    name: "PMR446 ch",
    mode: "nfm",
    bandwidthHz: 12_500,
    firstHz: 446_006_250,
    stepHz: 12_500,
    count: 16,
    note: "Channels 1-16, analog 12.5 kHz. Digital dPMR/DMR use their own raster.",
    source: SRC_BNETZA,
  },
  {
    kind: "list",
    id: "freenet",
    category: "freenet",
    name: "Freenet",
    mode: "nfm",
    bandwidthHz: 12_500,
    source: SRC_BNETZA,
    channels: [149.025, 149.0375, 149.05, 149.0875, 149.1, 149.1125].map((f, i) => ({
      label: String(i + 1),
      freqHz: Math.round(f * 1e6),
    })),
  },
  {
    kind: "grid",
    id: "lpd433",
    category: "lpd433",
    name: "LPD433 ch",
    mode: "nfm",
    bandwidthHz: 12_500,
    firstHz: 433_075_000,
    stepHz: 25_000,
    count: 69,
    note: "25 kHz raster, 10 mW.",
    source: SRC_BNETZA,
  },
  {
    kind: "list",
    id: "cb-standard",
    category: "cb",
    name: "CB ch",
    mode: "am",
    modeLabel: "AM / FM",
    bandwidthHz: 10_000,
    note: "AM or FM. Channel 9 is the emergency channel, channel 19 the road-traffic channel.",
    source: SRC_BNETZA,
    channels: CB_STANDARD_KHZ.map((k, i) => ({
      label: String(i + 1),
      freqHz: khz(k),
      ...(i === 8 ? { note: "Emergency channel (AM / FM)." } : {}),
      ...(i === 18 ? { note: "Road-traffic channel (AM / FM)." } : {}),
    })),
  },
  {
    kind: "grid",
    id: "cb-extended",
    category: "cb",
    name: "CB ch",
    mode: "nfm",
    modeLabel: "FM only",
    bandwidthHz: 12_500,
    firstHz: khz(26565),
    stepHz: khz(10),
    count: 40,
    firstNumber: 41,
    note: "German extension, channels 41-80, FM only.",
    source: SRC_BNETZA,
  },
  {
    kind: "list",
    id: "ism-433",
    category: "srd",
    name: "ISM 433.92",
    source: SRC_BNETZA,
    channels: [{ label: "ISM", freqHz: khz(433920), note: "Sensors, remote controls (OOK/FSK)." }],
  },
  {
    kind: "list",
    id: "air-guard",
    category: "aviation",
    name: "Aviation",
    mode: "am",
    bandwidthHz: 8_000,
    source: SRC_ITU,
    channels: [
      {
        label: "121.5",
        freqHz: khz(121500),
        name: "Aviation emergency (guard)",
        note: "International distress frequency; monitored, do not transmit.",
      },
    ],
  },
  {
    kind: "list",
    id: "adsb",
    category: "aviation",
    name: "ADS-B",
    modeLabel: "PPM 1 Mb/s",
    bandwidthHz: 2_000_000,
    note: "Mode S extended squitter; needs a dump1090-style decoder.",
    source: SRC_ITU,
    channels: [{ label: "ADS-B", freqHz: mhz(1090), name: "ADS-B 1090 MHz" }],
  },
  {
    kind: "list",
    id: "marine",
    category: "marine",
    name: "Marine",
    mode: "nfm",
    bandwidthHz: 25_000,
    source: SRC_MARINE,
    channels: [
      { label: "06", freqHz: khz(156300), name: "Marine ch 06", note: "Intership safety." },
      { label: "70", freqHz: khz(156525), name: "Marine ch 70", note: "DSC (digital, not voice)." },
      { label: "13", freqHz: khz(156650), name: "Marine ch 13", note: "Bridge-to-bridge." },
      {
        label: "16",
        freqHz: khz(156800),
        name: "Marine ch 16",
        note: "Distress, safety and calling.",
      },
    ],
  },
  {
    kind: "list",
    id: "ais",
    category: "marine",
    name: "AIS",
    modeLabel: "GMSK 9.6 kb/s",
    bandwidthHz: 25_000,
    note: "Ship tracking; needs an AIS decoder.",
    source: SRC_MARINE,
    channels: [
      { label: "AIS1", freqHz: khz(161975), name: "AIS 1 (87B)" },
      { label: "AIS2", freqHz: khz(162025), name: "AIS 2 (88B)" },
    ],
  },
];
