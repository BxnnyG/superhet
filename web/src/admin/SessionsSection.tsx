import { useCallback, useState } from "react";
import { describeUserAgent, formatDuration, formatMhz, formatRelative } from "../format";
import { describeHttpError } from "../http";
import { Badge, Button, ConfirmDialog, EmptyState, RefreshIcon, Toggle, toasts } from "../ui";
import type { AdminApi, LiveSession } from "./api";
import { Card, LoadState, RoleBadge, SectionHeader, useNow } from "./common";
import { useLoad } from "./useLoad";

const REFRESH_MS = 5000;

const KIND_LABEL: Record<string, string> = {
  spectrum: "Spectrum",
  audio: "Audio",
  state: "State",
  events: "Events",
};

export function sessionLabel(s: LiveSession): string {
  return s.kind === "guest" ? `Guest ${s.sid.slice(2, 8)}` : (s.username ?? s.sid);
}

export function SessionsSection({ api }: { api: AdminApi }) {
  const [auto, setAuto] = useState(true);
  const load = useCallback((s: AbortSignal) => api.listSessions(s), [api]);
  const sessions = useLoad(load, auto ? REFRESH_MS : 0);
  const [kicking, setKicking] = useState<LiveSession | null>(null);
  const [busy, setBusy] = useState(false);
  const now = useNow(5000);

  const kick = async (s: LiveSession) => {
    setBusy(true);
    try {
      await api.kickSession(s.sid);
      toasts.push({ tone: "success", title: `${sessionLabel(s)} was signed out` });
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: "Could not kick the session",
        body: describeHttpError(e),
      });
    } finally {
      setBusy(false);
      setKicking(null);
      sessions.reload();
    }
  };

  const list = sessions.data ?? [];
  return (
    <div>
      <SectionHeader
        title="Live sessions"
        description="Everyone who is signed in right now, with what they are tuned to. Kicking ends the session and closes its sockets."
        actions={
          <>
            <Toggle label="Auto-refresh" checked={auto} onChange={setAuto} />
            <Button icon={<RefreshIcon size={14} />} onClick={sessions.reload}>
              Refresh
            </Button>
          </>
        }
      />
      <Card title="Sessions" description={sessions.data ? `${list.length} online` : undefined}>
        <LoadState loading={sessions.loading} error={sessions.error} onRetry={sessions.reload} />
        {sessions.data && list.length === 0 && (
          <div className="p-4">
            <EmptyState title="Nobody is signed in" />
          </div>
        )}
        {sessions.data && list.length > 0 && (
          <ul className="divide-y divide-border" aria-label="Live sessions">
            {list.map((s) => (
              <li
                key={s.sid}
                className="flex flex-col gap-3 px-4 py-3"
                data-testid={`session-${s.sid}`}
              >
                <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-sm font-medium">{sessionLabel(s)}</span>
                      <RoleBadge role={s.role} guest={s.kind === "guest"} />
                    </div>
                    <p className="mt-0.5 text-xs text-fg-muted">
                      <span className="font-mono">{s.ip ?? "address not stored"}</span>
                      {" · "}
                      {describeUserAgent(s.userAgent)}
                    </p>
                    <p className="text-xs text-fg-subtle">
                      Signed in {formatRelative(s.createdAtMs, now)}
                      {s.kind === "guest" && s.idleSecs !== null
                        ? ` · idle ${formatDuration(s.idleSecs)}`
                        : s.lastSeenMs !== null
                          ? ` · last request ${formatRelative(s.lastSeenMs, now)}`
                          : ""}
                    </p>
                  </div>
                  <Button size="sm" variant="danger" onClick={() => setKicking(s)}>
                    Kick
                  </Button>
                </div>
                <div className="flex flex-wrap gap-1.5" aria-label="Open connections">
                  {s.connections.length === 0 && (
                    <span className="text-xs text-fg-subtle">No open connection</span>
                  )}
                  {countKinds(s).map(([kind, n]) => (
                    <Badge key={kind} tone="neutral" dot>
                      {KIND_LABEL[kind] ?? kind}
                      {n > 1 ? ` ×${n}` : ""}
                    </Badge>
                  ))}
                </div>
                {s.vfos.length > 0 && (
                  <ul className="flex flex-wrap gap-1.5" aria-label="Receivers">
                    {s.vfos.map((v) => (
                      <li
                        key={`${v.device}-${v.id}`}
                        className="rounded-md border border-border bg-surface-2 px-2 py-1 text-xs"
                      >
                        <span className="font-mono num">{formatMhz(v.freqHz)} MHz</span>
                        <span className="text-fg-muted"> · {v.mode.toUpperCase()}</span>
                        <span className="text-fg-subtle">
                          {" · "}
                          {v.device} #{v.id}
                          {v.status === "paused" ? " · paused" : ""}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ul>
        )}
      </Card>
      {kicking && (
        <ConfirmDialog
          title={`Sign out ${sessionLabel(kicking)}?`}
          confirmLabel="Kick"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setKicking(null)}
          onConfirm={() => void kick(kicking)}
        >
          {kicking.kind === "guest"
            ? "The guest session ends and its receivers are removed. They can start a new one if guest access stays on."
            : "This session ends and its sockets close. The account can sign in again; disable it under Users to keep it out."}
        </ConfirmDialog>
      )}
    </div>
  );
}

function countKinds(s: LiveSession): [string, number][] {
  const counts = new Map<string, number>();
  for (const c of s.connections) counts.set(c.kind, (counts.get(c.kind) ?? 0) + 1);
  return [...counts.entries()];
}
