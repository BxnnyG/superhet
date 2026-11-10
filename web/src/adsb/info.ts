// What the imported aircraft database knows about the aircraft in view (registration, type,
// airline), fetched in small batches from `GET /api/v1/adsb/lookup` (a local database query on the
// Superhet server; nothing goes to a third party) and cached in the browser.

import { isRec } from "../spectrum/api";
import type { AdsbApi } from "./api";

export interface AircraftInfo {
  registration: string | null;
  /** ICAO type designator, e.g. "A359". */
  typeCode: string | null;
  /** e.g. "AIRBUS A-350-900". */
  typeDesc: string | null;
  /** ICAO Doc 8643 class, e.g. "L2J". */
  typeClass: string | null;
  wake: string | null;
  military: boolean;
}

export interface OperatorInfo {
  name: string;
  country: string | null;
  /** Radio telephony callsign, e.g. "LUFTHANSA". */
  callsign: string | null;
}

export interface LookupReply {
  aircraft: Map<string, AircraftInfo>;
  operators: Map<string, OperatorInfo>;
}

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);

export function parseLookup(raw: unknown): LookupReply {
  const out: LookupReply = { aircraft: new Map(), operators: new Map() };
  if (!isRec(raw)) return out;
  if (isRec(raw.aircraft)) {
    for (const [icao, v] of Object.entries(raw.aircraft)) {
      if (!isRec(v)) continue;
      out.aircraft.set(icao.toLowerCase(), {
        registration: str(v.reg),
        typeCode: str(v.type),
        typeDesc: str(v.type_desc),
        typeClass: str(v.class),
        wake: str(v.wake),
        military: v.military === true,
      });
    }
  }
  if (isRec(raw.operators)) {
    for (const [code, v] of Object.entries(raw.operators)) {
      if (!isRec(v)) continue;
      const name = str(v.name);
      if (name === null) continue;
      out.operators.set(code.toUpperCase(), {
        name,
        country: str(v.country),
        callsign: str(v.callsign),
      });
    }
  }
  return out;
}

/** Aircraft or airlines the database did not know are asked again after this long. */
export const MISS_TTL_MS = 5 * 60_000;
/** Entries kept at most (a busy day sees a few thousand aircraft). */
const MAX_ENTRIES = 5_000;
/** Addresses per request (the server allows 400). */
const BATCH = 100;

interface Slot<T> {
  value: T | null;
  at: number;
}

/** The lookup results as an external store: `version` changes whenever something arrived. */
export class InfoCache {
  private readonly aircraft = new Map<string, Slot<AircraftInfo>>();
  private readonly operators = new Map<string, Slot<OperatorInfo>>();
  private wantAircraft = new Set<string>();
  private wantOperators = new Set<string>();
  private timer: ReturnType<typeof setTimeout> | undefined;
  private failedAt = 0;
  private version = 0;
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly api: Pick<AdsbApi, "lookup">,
    private readonly debounceMs = 150,
  ) {}

  readonly subscribe = (fn: () => void): (() => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };

  readonly getSnapshot = (): number => this.version;

  aircraftInfo(icao: string): AircraftInfo | null {
    return this.aircraft.get(icao)?.value ?? null;
  }

  operatorInfo(code: string | null): OperatorInfo | null {
    return code === null ? null : (this.operators.get(code)?.value ?? null);
  }

  private known<T>(map: Map<string, Slot<T>>, key: string, now: number): boolean {
    const slot = map.get(key);
    return slot !== undefined && (slot.value !== null || now - slot.at < MISS_TTL_MS);
  }

  /** Ask for what is not cached yet; results arrive after a short debounce. */
  request(icaos: readonly string[], operatorCodes: readonly string[]): void {
    const now = Date.now();
    let added = false;
    for (const icao of icaos) {
      if (/^[0-9a-f]{6}$/.test(icao) && !this.known(this.aircraft, icao, now)) {
        this.wantAircraft.add(icao);
        added = true;
      }
    }
    for (const code of operatorCodes) {
      if (!this.known(this.operators, code, now)) {
        this.wantOperators.add(code);
        added = true;
      }
    }
    if (added && this.timer === undefined) {
      this.timer = setTimeout(() => void this.flush(), this.debounceMs);
    }
  }

  /** Forget everything (the database was replaced or removed). */
  clear(): void {
    this.aircraft.clear();
    this.operators.clear();
    this.bump();
  }

  dispose(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
    this.listeners.clear();
  }

  private bump(): void {
    this.version += 1;
    for (const fn of [...this.listeners]) fn();
  }

  private async flush(): Promise<void> {
    this.timer = undefined;
    // After a failure, stay quiet for a while instead of asking every second.
    if (Date.now() - this.failedAt < 30_000) {
      this.wantAircraft.clear();
      this.wantOperators.clear();
      return;
    }
    const icaos = [...this.wantAircraft];
    const codes = [...this.wantOperators];
    this.wantAircraft = new Set();
    this.wantOperators = new Set();
    if (this.aircraft.size > MAX_ENTRIES) this.aircraft.clear();
    try {
      for (let i = 0; i < Math.max(icaos.length, codes.length); i += BATCH) {
        const a = icaos.slice(i, i + BATCH);
        const o = codes.slice(i, i + BATCH);
        const reply = await this.api.lookup(a, o);
        const at = Date.now();
        for (const icao of a) {
          this.aircraft.set(icao, { value: reply.aircraft.get(icao) ?? null, at });
        }
        for (const code of o) {
          this.operators.set(code, { value: reply.operators.get(code) ?? null, at });
        }
      }
    } catch {
      this.failedAt = Date.now();
    }
    this.bump();
  }
}

/** Source and licence of the imported database, as `GET /api/v1/adsb/aircraft-db` reports it. */
export interface DatabaseSource {
  source: string;
  sourceUrl: string;
  licence: string;
  licenceUrl: string;
  version: string;
  origin: "download" | "file";
  aircraft: number;
  types: number;
  operators: number;
  importedMs: number;
}

export interface DatabaseOffer {
  source: string;
  sourceUrl: string;
  licence: string;
  licenceUrl: string;
  downloadUrl: string;
  /** `SUPERHET_AIRCRAFT_DB_FILE`, if set. */
  file: string | null;
  /** A downloaded archive is on disk (re-import without the network). */
  archive: boolean;
}

export type JobState = "idle" | "downloading" | "importing" | "done" | "failed";

export interface DatabaseStatus {
  database: DatabaseSource | null;
  offer: DatabaseOffer;
  job: { state: JobState; message: string; finishedMs: number | null };
}

const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);

export function parseDatabaseStatus(raw: unknown): DatabaseStatus | null {
  if (!isRec(raw) || !isRec(raw.offer) || !isRec(raw.job)) return null;
  const o = raw.offer;
  const db = isRec(raw.database) ? raw.database : null;
  const job = raw.job;
  const jobState: JobState =
    (["idle", "downloading", "importing", "done", "failed"] as const).find(
      (s) => s === job.state,
    ) ?? "idle";
  return {
    database:
      db === null
        ? null
        : {
            source: str(db.source) ?? "",
            sourceUrl: str(db.source_url) ?? "",
            licence: str(db.licence) ?? "",
            licenceUrl: str(db.licence_url) ?? "",
            version: str(db.version) ?? "",
            origin: db.origin === "file" ? "file" : "download",
            aircraft: num(db.aircraft),
            types: num(db.types),
            operators: num(db.operators),
            importedMs: num(db.imported_ms),
          },
    offer: {
      source: str(o.source) ?? "",
      sourceUrl: str(o.source_url) ?? "",
      licence: str(o.licence) ?? "",
      licenceUrl: str(o.licence_url) ?? "",
      downloadUrl: str(o.download_url) ?? "",
      file: str(o.file),
      archive: o.archive === true,
    },
    job: {
      state: jobState,
      message: str(job.message) ?? "",
      finishedMs: typeof job.finished_ms === "number" ? job.finished_ms : null,
    },
  };
}
