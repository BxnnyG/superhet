import { useCallback, useMemo, useState } from "react";
import { describeHttpError } from "../http";
import { Alert, Badge, Button, ConfirmDialog, PasswordField, Toggle, toasts } from "../ui";
import type { BackupApi, BackupArea, ImportReport, ScheduleStatus } from "./backupApi";
import { createBackupApi } from "./backupApi";
import { Card, LoadState, SectionHeader } from "./common";
import { useLoad } from "./useLoad";

function formatBytes(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(1)} GiB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MiB`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KiB`;
  return `${n} B`;
}

function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

function AreaPicker({
  areas,
  selected,
  onChange,
  idPrefix,
}: {
  areas: BackupArea[];
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  idPrefix: string;
}) {
  return (
    <ul className="grid gap-2 sm:grid-cols-2">
      {areas.map((a) => (
        <li key={a.id} className="flex items-start gap-2">
          <input
            id={`${idPrefix}-${a.id}`}
            type="checkbox"
            className="mt-1"
            checked={selected.has(a.id)}
            onChange={(e) => {
              const next = new Set(selected);
              if (e.target.checked) next.add(a.id);
              else next.delete(a.id);
              onChange(next);
            }}
          />
          <label htmlFor={`${idPrefix}-${a.id}`} className="text-sm">
            {a.title}
            <span className="block text-xs text-fg-muted">{a.description}</span>
          </label>
        </li>
      ))}
    </ul>
  );
}

function ScheduleCard({ status }: { status: ScheduleStatus | null }) {
  if (status === null || !status.enabled) {
    return (
      <Card
        title="Scheduled backups"
        description="Off. Set SUPERHET_BACKUP_DIR (and optionally SUPERHET_BACKUP_SCHEDULE, SUPERHET_BACKUP_KEEP) on the worker service to write archives automatically."
      >
        {status?.error ? (
          <Alert tone="danger" className="m-4">
            {status.error}
          </Alert>
        ) : null}
      </Card>
    );
  }
  return (
    <Card
      title="Scheduled backups"
      description={`Schedule "${status.schedule}" (UTC), keeping the newest ${status.keep} in ${status.dir} inside the worker.`}
    >
      <div className="flex flex-col gap-3 p-4 text-sm" data-testid="backup-schedule">
        <div className="flex flex-wrap gap-2">
          <Badge tone={status.encrypted ? "success" : "neutral"}>
            {status.encrypted ? "encrypted" : "not encrypted"}
          </Badge>
          {status.credentials && <Badge tone="warn">includes credentials</Badge>}
          {status.history && <Badge tone="neutral">includes history</Badge>}
        </div>
        <p>
          Next run:{" "}
          {status.nextRun === null ? "none" : new Date(status.nextRun * 1000).toLocaleString()}
        </p>
        {status.last && (
          <p>
            Last run: {new Date(status.last.at * 1000).toLocaleString()} -{" "}
            {status.last.ok ? `ok (${status.last.file})` : `failed: ${status.last.error}`}
          </p>
        )}
        {status.files.length > 0 && (
          <ul className="text-xs text-fg-muted">
            {status.files.map((f) => (
              <li key={f.name} className="font-mono">
                {f.name} ({formatBytes(f.bytes)})
              </li>
            ))}
          </ul>
        )}
      </div>
    </Card>
  );
}

function ReportView({ report }: { report: ImportReport }) {
  return (
    <div className="flex flex-col gap-3 p-4 text-sm" data-testid="backup-report">
      <p>
        Backup from {report.createdAt || "unknown time"} by {report.createdBy || "unknown"}{" "}
        (Superhet {report.appVersion}, schema {report.schemaVersion}
        {report.schemaVersion !== report.currentSchemaVersion
          ? `, this install: ${report.currentSchemaVersion}`
          : ""}
        ){report.includesCredentials ? ", contains credentials" : ""}.
      </p>
      {report.conflicts.map((c) => (
        <Alert key={c} tone="warn">
          {c}
        </Alert>
      ))}
      {report.warnings.map((w) => (
        <Alert key={w} tone="info">
          {w}
        </Alert>
      ))}
      <table className="w-full text-left text-xs">
        <thead className="text-fg-muted">
          <tr>
            <th className="py-1">Table</th>
            <th>In backup</th>
            <th>Now</th>
            <th>Removed</th>
            <th>Written</th>
            <th>Skipped</th>
          </tr>
        </thead>
        <tbody>
          {report.tables.map((t) => (
            <tr key={t.table} className="border-t border-border">
              <td className="py-1 font-mono">{t.table}</td>
              <td>{t.inArchive}</td>
              <td>{t.existing}</td>
              <td>{t.deleted}</td>
              <td>{t.inserted + t.updated}</td>
              <td>{t.skipped}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {report.filesInArchive > 0 && (
        <p>
          {report.filesInArchive} recording file(s) in the backup
          {report.dryRun ? "" : `, ${report.filesRestored} restored`}.
        </p>
      )}
    </div>
  );
}

/** Backup section of the admin console: create and download, check and restore, schedule status. */
export function BackupSection({ api: injected }: { api?: BackupApi }) {
  const api = useMemo(() => injected ?? createBackupApi(), [injected]);
  const load = useCallback((s: AbortSignal) => api.overview(s), [api]);
  const loaded = useLoad(load);
  const overview = loaded.data;

  const [exportAreas, setExportAreas] = useState<Set<string> | null>(null);
  const [credentials, setCredentials] = useState(false);
  const [exportPass, setExportPass] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [file, setFile] = useState<File | null>(null);
  const [importPass, setImportPass] = useState("");
  const [overwriteSelf, setOverwriteSelf] = useState(false);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [confirming, setConfirming] = useState(false);

  const areas = overview?.areas ?? [];
  const chosen = exportAreas ?? new Set(areas.filter((a) => !a.optional).map((a) => a.id));
  const minPass = overview?.minPassphraseLength ?? 8;
  const passOk = exportPass === "" || exportPass.length >= minPass;
  const hasCredentialAreas = areas.some((a) => a.credentials !== "");

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const { blob, filename } = await api.exportArchive({
        areas: [...chosen],
        includeCredentials: credentials,
        passphrase: exportPass,
      });
      saveBlob(blob, filename);
      toasts.push({ tone: "success", title: "Backup created", body: filename });
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  const run = async (dryRun: boolean) => {
    if (file === null) return;
    setBusy(true);
    setError(null);
    try {
      const result = await api.importArchive(file, {
        dryRun,
        overwriteSelf,
        areas: [],
        passphrase: importPass,
      });
      setReport(result);
      if (!dryRun) {
        toasts.push({ tone: "success", title: "Backup restored" });
        loaded.reload();
      }
    } catch (e) {
      setReport(null);
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Backup"
        description="Export settings, accounts, bookmarks, profiles, policies, rules and more as one archive, and restore it later or on another install. The aircraft database is not included (download it again)."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {error !== null && (
        <Alert tone="danger" className="mb-4">
          {error}
        </Alert>
      )}
      {overview && (
        <div className="flex flex-col gap-6">
          <Card
            title="Create a backup"
            description="Downloads one zip archive. Optionally sealed with a passphrase (AES-256-GCM)."
          >
            <div className="flex flex-col gap-4 p-4">
              <AreaPicker
                areas={areas}
                selected={chosen}
                onChange={setExportAreas}
                idPrefix="export-area"
              />
              {hasCredentialAreas && (
                <div className="flex items-start gap-3">
                  <Toggle
                    checked={credentials}
                    onChange={setCredentials}
                    label="Include credentials"
                  />
                  <p className="text-xs text-fg-muted">
                    Adds password hashes, open invites, notification target secrets and the Web Push
                    key. Without it restored accounts cannot sign in until you reset their password.
                    Set a passphrase when you tick this.
                  </p>
                </div>
              )}
              <PasswordField
                label="Passphrase (optional)"
                value={exportPass}
                onChange={(e) => setExportPass(e.target.value)}
                autoComplete="new-password"
              />
              {!passOk && (
                <p className="text-xs text-danger">
                  The passphrase needs at least {minPass} characters.
                </p>
              )}
              <div>
                <Button
                  variant="primary"
                  disabled={busy || chosen.size === 0 || !passOk}
                  onClick={() => void create()}
                >
                  Create and download
                </Button>
              </div>
            </div>
          </Card>

          <Card
            title="Restore a backup"
            description={`Upload an archive, check what it would change, then restore. Restoring replaces the data of the areas in the archive in one transaction; your own account is only overwritten when you confirm. Upload limit ${formatBytes(overview.maxUploadBytes)}.`}
          >
            <div className="flex flex-col gap-4 p-4">
              <input
                type="file"
                aria-label="Backup file"
                accept=".zip,.enc,application/zip,application/octet-stream"
                onChange={(e) => {
                  setFile(e.target.files?.[0] ?? null);
                  setReport(null);
                }}
              />
              <PasswordField
                label="Passphrase (if the backup is encrypted)"
                value={importPass}
                onChange={(e) => setImportPass(e.target.value)}
                autoComplete="off"
              />
              <div className="flex items-center gap-3">
                <Toggle
                  checked={overwriteSelf}
                  onChange={(v) => {
                    setOverwriteSelf(v);
                    setReport(null);
                  }}
                  label="Overwrite my own account with the backup's version"
                />
              </div>
              <div className="flex gap-2">
                <Button disabled={busy || file === null} onClick={() => void run(true)}>
                  Check backup
                </Button>
                <Button
                  variant="primary"
                  disabled={busy || file === null || report === null || !report.dryRun}
                  onClick={() => setConfirming(true)}
                >
                  Restore
                </Button>
              </div>
            </div>
            {report && <ReportView report={report} />}
          </Card>

          <ScheduleCard status={overview.schedule} />
        </div>
      )}
      {confirming && (
        <ConfirmDialog
          title="Restore this backup?"
          confirmLabel="Restore"
          confirmVariant="danger"
          busy={busy}
          onConfirm={() => void run(false)}
          onCancel={() => setConfirming(false)}
        >
          The data of the checked areas is replaced. Other accounts are signed out. This cannot be
          undone; create a backup first if unsure.
        </ConfirmDialog>
      )}
    </div>
  );
}
