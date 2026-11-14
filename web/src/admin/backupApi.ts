// Backup and restore (docs/backup.md, ADR-0030): overview, export download, import dry run / apply.

import { apiFetch } from "../auth/session";
import { HttpError, isJson, request, type FetchLike, type Json } from "../http";

export interface BackupArea {
  id: string;
  title: string;
  description: string;
  /** Not part of the default selection (history, large files). */
  optional: boolean;
  /** What "include credentials" adds to this area; "" when nothing. */
  credentials: string;
}

export interface BackupFileInfo {
  name: string;
  bytes: number;
}

/** State of the scheduled backups, kept by the worker. */
export interface ScheduleStatus {
  enabled: boolean;
  dir: string;
  schedule: string;
  keep: number;
  encrypted: boolean;
  credentials: boolean;
  history: boolean;
  /** Unix seconds of the next run, or null. */
  nextRun: number | null;
  last: { at: number; ok: boolean; error: string; file: string } | null;
  files: BackupFileInfo[];
  /** Why the job is off although it was configured ("" when simply not configured). */
  error: string;
}

export interface BackupOverview {
  areas: BackupArea[];
  maxUploadBytes: number;
  minPassphraseLength: number;
  schedule: ScheduleStatus | null;
}

export interface TableReport {
  table: string;
  area: string;
  inArchive: number;
  existing: number;
  deleted: number;
  inserted: number;
  updated: number;
  skipped: number;
}

export interface ImportReport {
  dryRun: boolean;
  createdAt: string;
  createdBy: string;
  appVersion: string;
  schemaVersion: number;
  currentSchemaVersion: number;
  includesCredentials: boolean;
  areas: string[];
  tables: TableReport[];
  filesInArchive: number;
  filesRestored: number;
  warnings: string[];
  conflicts: string[];
}

export interface ExportRequest {
  areas: string[];
  includeCredentials: boolean;
  passphrase: string;
}

export interface ImportRequest {
  dryRun: boolean;
  overwriteSelf: boolean;
  areas: string[];
  passphrase: string;
}

export interface BackupApi {
  overview(signal?: AbortSignal): Promise<BackupOverview>;
  exportArchive(req: ExportRequest): Promise<{ blob: Blob; filename: string }>;
  importArchive(file: Blob, req: ImportRequest): Promise<ImportReport>;
}

const str = (v: unknown, d = ""): string => (typeof v === "string" ? v : d);
const num = (v: unknown, d = 0): number => (typeof v === "number" && Number.isFinite(v) ? v : d);
const strs = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x) => typeof x === "string") : [];
const list = (v: unknown): Json[] => (Array.isArray(v) ? v.filter(isJson) : []);

function parseSchedule(raw: unknown): ScheduleStatus | null {
  if (!isJson(raw)) return null;
  const last = isJson(raw.last)
    ? {
        at: num(raw.last.at),
        ok: raw.last.ok === true,
        error: str(raw.last.error),
        file: isJson(raw.last.file) ? str(raw.last.file.name) : "",
      }
    : null;
  return {
    enabled: raw.enabled === true,
    dir: str(raw.dir),
    schedule: str(raw.schedule),
    keep: num(raw.keep),
    encrypted: raw.encrypted === true,
    credentials: raw.credentials === true,
    history: raw.history === true,
    nextRun: typeof raw.next_run === "number" ? raw.next_run : null,
    last,
    files: list(raw.files).map((f) => ({ name: str(f.name), bytes: num(f.bytes) })),
    error: str(raw.error),
  };
}

export function parseOverview(raw: unknown): BackupOverview {
  const o = isJson(raw) ? raw : {};
  return {
    areas: list(o.areas).map((a) => ({
      id: str(a.id),
      title: str(a.title),
      description: str(a.description),
      optional: a.optional === true,
      credentials: str(a.credentials),
    })),
    maxUploadBytes: num(o.max_upload_bytes),
    minPassphraseLength: num(o.min_passphrase_length, 8),
    schedule: parseSchedule(o.schedule),
  };
}

export function parseReport(raw: unknown): ImportReport {
  const r = isJson(raw) && isJson(raw.report) ? raw.report : {};
  const b = isJson(r.backup) ? r.backup : {};
  return {
    dryRun: r.dry_run === true,
    createdAt: str(b.created_at),
    createdBy: str(b.created_by),
    appVersion: str(b.app_version),
    schemaVersion: num(b.schema_version),
    currentSchemaVersion: num(r.current_schema_version),
    includesCredentials: b.includes_credentials === true,
    areas: strs(r.areas),
    tables: list(r.tables).map((t) => ({
      table: str(t.table),
      area: str(t.area),
      inArchive: num(t.in_archive),
      existing: num(t.existing),
      deleted: num(t.deleted),
      inserted: num(t.inserted),
      updated: num(t.updated),
      skipped: num(t.skipped),
    })),
    filesInArchive: num(r.files_in_archive),
    filesRestored: num(r.files_restored),
    warnings: strs(r.warnings),
    conflicts: strs(r.conflicts),
  };
}

async function fail(res: Response): Promise<never> {
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    body = undefined;
  }
  const code = isJson(body) && typeof body.error === "string" ? body.error : "";
  const msg = isJson(body) && typeof body.message === "string" ? body.message : "";
  throw new HttpError(res.status, code, msg || `Request failed (HTTP ${res.status}).`);
}

export function createBackupApi(fetchImpl: FetchLike = apiFetch): BackupApi {
  return {
    async overview(signal) {
      const reply = await request("GET", "/api/v1/admin/backup", { signal, fetchImpl });
      return parseOverview(reply.body);
    },
    async exportArchive(req) {
      const res = await fetchImpl("/api/v1/admin/backup/export", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          areas: req.areas,
          include_credentials: req.includeCredentials,
          passphrase: req.passphrase === "" ? undefined : req.passphrase,
        }),
      });
      if (!res.ok) return fail(res);
      const disposition = res.headers.get("content-disposition") ?? "";
      const filename = /filename="([^"]+)"/.exec(disposition)?.[1] ?? "superhet-backup.zip";
      return { blob: await res.blob(), filename };
    },
    async importArchive(file, req) {
      const q = new URLSearchParams();
      if (req.dryRun) q.set("dry_run", "true");
      if (req.overwriteSelf) q.set("overwrite_self", "true");
      if (req.areas.length > 0) q.set("areas", req.areas.join(","));
      const headers: Record<string, string> = { "Content-Type": "application/octet-stream" };
      if (req.passphrase !== "")
        headers["X-Backup-Passphrase"] = encodeURIComponent(req.passphrase);
      const suffix = q.size > 0 ? `?${q.toString()}` : "";
      const res = await fetchImpl(`/api/v1/admin/backup/import${suffix}`, {
        method: "POST",
        headers,
        body: file,
      });
      if (!res.ok) return fail(res);
      return parseReport(await res.json());
    },
  };
}
