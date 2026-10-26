import { useCallback, useState } from "react";
import { formatDateTime } from "../format";
import { describeHttpError } from "../http";
import { Alert, NumberField, Select, toasts } from "../ui";
import type { AdminApi, AuditSettings, AuditStats, IpMode } from "./api";
import { Card, LoadState, SaveBar, SectionHeader } from "./common";
import { useDraft } from "./useDraft";
import { useLoad } from "./useLoad";

const IP_OPTIONS: { value: IpMode; label: string }[] = [
  { value: "full", label: "Store the full address" },
  { value: "truncated", label: "Truncate (IPv4 /24, IPv6 /48)" },
  { value: "none", label: "Do not store" },
];

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(1)} MiB`;
  return `${(n / 1024 ** 3).toFixed(2)} GiB`;
}

export function AuditSettingsSection({ api }: { api: AdminApi }) {
  const load = useCallback((s: AbortSignal) => api.getAuditSettings(s), [api]);
  const loaded = useLoad(load);
  const [saved, setSaved] = useState<AuditSettings | null>(null);
  const draft = useDraft(saved ?? loaded.data?.settings ?? null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const s = draft.value;
  const stats: AuditStats | null = loaded.data?.stats ?? null;

  const save = async () => {
    if (!s) return;
    setBusy(true);
    setError(null);
    try {
      setSaved(await api.putAuditSettings(s));
      toasts.push({ tone: "success", title: "Audit settings saved" });
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };
  const update = (patch: Partial<AuditSettings>) => s && draft.set({ ...s, ...patch });

  return (
    <div>
      <SectionHeader
        title="Audit settings"
        description="How long audit entries are kept and how much of a visitor's address is stored. The address mode applies when an entry is written; older entries keep what they have."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {s && (
        <div className="flex flex-col gap-6">
          <Card
            title="Retention"
            description="Entries older than this are deleted hourly. 0 keeps them forever."
          >
            <div className="grid gap-4 p-4 sm:grid-cols-2">
              <NumberField
                label="Account and system entries"
                unit="days"
                min={0}
                max={36_500}
                value={s.retentionUserDays}
                onCommit={(retentionUserDays) => update({ retentionUserDays })}
              />
              <NumberField
                label="Guest entries"
                unit="days"
                min={0}
                max={36_500}
                value={s.retentionGuestDays}
                onCommit={(retentionGuestDays) => update({ retentionGuestDays })}
              />
            </div>
            {s.retentionGuestDays === 0 && (
              <div className="px-4 pb-4">
                <Alert tone="warn">
                  Keeping guest entries forever is discouraged: they contain visitors&apos;
                  addresses.
                </Alert>
              </div>
            )}
          </Card>

          <Card
            title="Client addresses"
            description="Addresses and browser strings of visitors are personal data."
          >
            <div className="grid gap-4 p-4 sm:grid-cols-2">
              <Select
                label="Account addresses"
                value={s.ipModeUser}
                onChange={(v) => update({ ipModeUser: v as IpMode })}
                options={IP_OPTIONS}
              />
              <Select
                label="Guest addresses"
                value={s.ipModeGuest}
                onChange={(v) => update({ ipModeGuest: v as IpMode })}
                options={IP_OPTIONS}
              />
            </div>
            <p className="px-4 pb-4 text-xs text-fg-subtle">
              The live session list shows addresses in the same mode. Behind a reverse proxy the
              address is only right with{" "}
              <code className="font-mono">SUPERHET_TRUST_PROXY=true</code>.
            </p>
          </Card>

          {stats && (
            <Card title="Stored">
              <dl className="grid gap-x-8 gap-y-2 p-4 text-sm sm:grid-cols-3">
                <div>
                  <dt className="text-xs text-fg-muted">Entries</dt>
                  <dd className="font-mono num">{stats.count.toLocaleString()}</dd>
                </div>
                <div>
                  <dt className="text-xs text-fg-muted">Oldest</dt>
                  <dd className="num">{stats.oldestMs ? formatDateTime(stats.oldestMs) : "—"}</dd>
                </div>
                <div>
                  <dt className="text-xs text-fg-muted">Size</dt>
                  <dd className="num">{formatBytes(stats.bytes)}</dd>
                </div>
              </dl>
            </Card>
          )}

          <Card className="sticky bottom-4 z-10 shadow-pop">
            <SaveBar
              dirty={draft.dirty}
              busy={busy}
              error={error}
              onSave={() => void save()}
              onDiscard={() => {
                draft.discard();
                setError(null);
              }}
            />
          </Card>
        </div>
      )}
    </div>
  );
}
