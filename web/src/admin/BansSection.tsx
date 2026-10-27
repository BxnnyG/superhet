import { useCallback, useState } from "react";
import { formatDateTime, formatRelative } from "../format";
import { describeHttpError } from "../http";
import {
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  PlusIcon,
  RefreshIcon,
  Toggle,
  toasts,
} from "../ui";
import type { AdminApi, Ban } from "./api";
import { BanDialog } from "./BanDialog";
import { Card, LoadState, SectionHeader, useNow } from "./common";
import { useLoad } from "./useLoad";

function expiry(ban: Ban, now: number): string {
  if (ban.state === "lifted" && ban.liftedAtMs !== null) {
    return `lifted ${formatRelative(ban.liftedAtMs, now)} by ${ban.liftedBy ?? "an admin"}`;
  }
  if (ban.expiresAtMs === null) return "permanent";
  return ban.state === "expired"
    ? `expired ${formatRelative(ban.expiresAtMs, now)}`
    : `until ${formatDateTime(ban.expiresAtMs)}`;
}

export function BansSection({ api }: { api: AdminApi }) {
  const [history, setHistory] = useState(false);
  const load = useCallback((s: AbortSignal) => api.listBans(history, s), [api, history]);
  const bans = useLoad(load);
  const [adding, setAdding] = useState(false);
  const [lifting, setLifting] = useState<Ban | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useNow(30000);

  const lift = async (ban: Ban) => {
    setBusy(true);
    try {
      await api.liftBan(ban.id);
      toasts.push({ tone: "success", title: `${ban.network} is no longer banned` });
    } catch (e) {
      toasts.push({ tone: "danger", title: "Could not lift the ban", body: describeHttpError(e) });
    } finally {
      setBusy(false);
      setLifting(null);
      bans.reload();
    }
  };

  const list = bans.data ?? [];
  return (
    <div>
      <SectionHeader
        title="Address bans"
        description="Banned addresses are refused before sign-in, guest entry and WebSocket upgrades. Ban from here, from a live session (Kick and ban) or from an audit row."
        actions={
          <>
            <Toggle label="Show history" checked={history} onChange={setHistory} />
            <Button icon={<RefreshIcon size={14} />} onClick={bans.reload}>
              Refresh
            </Button>
            <Button variant="primary" icon={<PlusIcon size={14} />} onClick={() => setAdding(true)}>
              Ban an address
            </Button>
          </>
        }
      />
      <Card
        title={history ? "All bans" : "Active bans"}
        description={bans.data ? `${list.length} listed` : undefined}
      >
        <LoadState loading={bans.loading} error={bans.error} onRetry={bans.reload} />
        {bans.data && list.length === 0 && (
          <div className="p-4">
            <EmptyState title="No bans">Nobody is blocked right now.</EmptyState>
          </div>
        )}
        {bans.data && list.length > 0 && (
          <ul className="divide-y divide-border" aria-label="Bans">
            {list.map((b) => (
              <li
                key={b.id}
                className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 px-4 py-3"
                data-testid={`ban-${b.id}`}
              >
                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-1.5">
                    <span className="font-mono text-sm font-medium">{b.network}</span>
                    {b.state !== "active" && <Badge>{b.state}</Badge>}
                  </div>
                  <p className="mt-0.5 text-xs text-fg-muted">
                    {b.reason ?? "No reason given"}
                    {" · "}
                    {expiry(b, now)}
                  </p>
                  <p className="text-xs text-fg-subtle">
                    Banned {formatRelative(b.createdAtMs, now)} by {b.createdBy}
                  </p>
                </div>
                {b.state === "active" && (
                  <Button size="sm" onClick={() => setLifting(b)}>
                    Lift
                  </Button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Card>
      {adding && (
        <BanDialog
          api={api}
          target={{ kind: "network", network: "" }}
          onClose={() => setAdding(false)}
          onDone={bans.reload}
        />
      )}
      {lifting && (
        <ConfirmDialog
          title={`Lift the ban on ${lifting.network}?`}
          confirmLabel="Lift ban"
          busy={busy}
          onCancel={() => setLifting(null)}
          onConfirm={() => void lift(lifting)}
        >
          The address can reach the server again at once.
        </ConfirmDialog>
      )}
    </div>
  );
}
