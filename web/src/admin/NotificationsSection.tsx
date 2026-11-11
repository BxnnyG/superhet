import { useCallback, useState } from "react";
import { describeHttpError } from "../http";
import { formatRelative } from "../format";
import {
  createNotificationsApi,
  type Delivery,
  type DeliveryStatus,
  type NotificationsApi,
  type Rule,
  type Target,
} from "../notifications/api";
import {
  Badge,
  Button,
  ConfirmDialog,
  EmptyState,
  PencilIcon,
  PlusIcon,
  RefreshIcon,
  SegmentedControl,
  Toggle,
  TrashIcon,
  IconButton,
  Select,
  toasts,
  type BadgeTone,
} from "../ui";
import { Card, LoadState, SectionHeader } from "./common";
import { RuleEditor } from "./RuleEditor";
import { TargetEditor } from "./TargetEditor";
import { useLoad } from "./useLoad";

type Tab = "rules" | "targets" | "log";

const TABS = [
  { value: "rules", label: "Rules" },
  { value: "targets", label: "Targets" },
  { value: "log", label: "Delivery log" },
] as const;

const STATUS_TONE: Record<DeliveryStatus, BadgeTone> = {
  sent: "success",
  pending: "warn",
  failed: "danger",
};

/**
 * Notifications: rules that fire a webhook, e-mail or browser push when a decoder event matches,
 * the targets they send to, and the delivery log with a "send test" button.
 */
export function NotificationsSection({ api: injected }: { api?: NotificationsApi }) {
  const [api] = useState(() => injected ?? createNotificationsApi());
  const [tab, setTab] = useState<Tab>("rules");
  return (
    <div>
      <SectionHeader
        title="Notifications"
        description="Get told when a decoder hears something you care about: a pager RIC or word, a callsign, a talkgroup, an aircraft. Messages go out through the worker; secrets are stored write-only."
      />
      <div className="mb-4">
        <SegmentedControl
          aria-label="Notification views"
          options={TABS}
          value={tab}
          onChange={setTab}
        />
      </div>
      {tab === "rules" && <RulesTab api={api} />}
      {tab === "targets" && <TargetsTab api={api} />}
      {tab === "log" && <LogTab api={api} />}
    </div>
  );
}

function useTargets(api: NotificationsApi) {
  return useLoad(useCallback((s: AbortSignal) => api.listTargets(s), [api]));
}

function RulesTab({ api }: { api: NotificationsApi }) {
  const rules = useLoad(useCallback((s: AbortSignal) => api.listRules(s), [api]));
  const targets = useTargets(api);
  const [editing, setEditing] = useState<Rule | "new" | null>(null);
  const [removing, setRemoving] = useState<Rule | null>(null);
  const [busy, setBusy] = useState(false);
  const names = new Map((targets.data ?? []).map((t) => [t.id, t.name]));

  const toggle = async (r: Rule, enabled: boolean) => {
    try {
      await api.updateRule(r.id, {
        name: r.name,
        enabled,
        source: r.source,
        kind: r.kind,
        conditions: r.conditions,
        cooldown_secs: r.cooldownSecs,
        max_per_hour: r.maxPerHour,
        quiet: r.quiet,
        title_template: r.titleTemplate,
        body_template: r.bodyTemplate,
        target_ids: r.targetIds,
      });
      rules.reload();
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: "Could not change the rule",
        body: describeHttpError(e),
      });
    }
  };
  const test = async (r: Rule) => {
    try {
      await api.testRule(r.id);
      toasts.push({
        tone: "success",
        title: "Test queued",
        body: "See the delivery log for the result.",
      });
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: "Could not queue the test",
        body: describeHttpError(e),
      });
    }
  };
  const remove = async () => {
    if (!removing) return;
    setBusy(true);
    try {
      await api.deleteRule(removing.id);
      setRemoving(null);
      rules.reload();
    } catch (e) {
      toasts.push({ tone: "danger", title: "Could not delete", body: describeHttpError(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      title="Rules"
      description="Evaluated for every decoder event, in the worker. A cooldown or hourly cap keeps a busy channel from flooding you."
      actions={
        <Button
          variant="primary"
          size="sm"
          icon={<PlusIcon size={14} />}
          onClick={() => setEditing("new")}
        >
          New rule
        </Button>
      }
    >
      <LoadState loading={rules.loading} error={rules.error} onRetry={rules.reload} />
      {rules.data && rules.data.length === 0 && (
        <div className="p-4">
          <EmptyState title="No rules yet">
            Create a target first, then a rule that sends to it.
          </EmptyState>
        </div>
      )}
      {rules.data && rules.data.length > 0 && (
        <ul className="divide-y divide-border" aria-label="Rules">
          {rules.data.map((r) => (
            <li
              key={r.id}
              className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3"
              data-testid={`rule-${r.id}`}
            >
              <div className="min-w-0 flex-1 basis-56">
                <div className="flex flex-wrap items-center gap-2 text-sm font-medium">
                  {r.name}
                  {r.quiet && (
                    <Badge>
                      quiet {r.quiet.start}-{r.quiet.end}
                    </Badge>
                  )}
                </div>
                <p className="truncate text-xs text-fg-muted">
                  {r.source ?? "any source"} · {r.kind ?? "any event"} · {r.conditions.length}{" "}
                  condition
                  {r.conditions.length === 1 ? "" : "s"} →{" "}
                  {r.targetIds.map((id) => names.get(id) ?? "?").join(", ") || "no target"}
                </p>
              </div>
              <Toggle
                label={`Enable ${r.name}`}
                hideLabel
                checked={r.enabled}
                onChange={(on) => void toggle(r, on)}
              />
              <Button size="sm" onClick={() => void test(r)} disabled={r.targetIds.length === 0}>
                Send test
              </Button>
              <IconButton label={`Edit ${r.name}`} size="sm" onClick={() => setEditing(r)}>
                <PencilIcon />
              </IconButton>
              <IconButton label={`Delete ${r.name}`} size="sm" onClick={() => setRemoving(r)}>
                <TrashIcon />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
      {editing && (
        <RuleEditor
          rule={editing === "new" ? null : editing}
          targets={targets.data ?? []}
          api={api}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            rules.reload();
          }}
        />
      )}
      {removing && (
        <ConfirmDialog
          title={`Delete ${removing.name}?`}
          confirmLabel="Delete"
          confirmVariant="danger"
          busy={busy}
          onConfirm={() => void remove()}
          onCancel={() => setRemoving(null)}
        >
          The rule stops firing. Its delivery log entries stay.
        </ConfirmDialog>
      )}
    </Card>
  );
}

function TargetsTab({ api }: { api: NotificationsApi }) {
  const targets = useTargets(api);
  const [editing, setEditing] = useState<Target | "new" | null>(null);
  const [removing, setRemoving] = useState<Target | null>(null);
  const [busy, setBusy] = useState(false);

  const test = async (t: Target) => {
    try {
      await api.testTarget(t.id);
      toasts.push({
        tone: "success",
        title: "Test queued",
        body: "See the delivery log for the result.",
      });
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: "Could not queue the test",
        body: describeHttpError(e),
      });
    }
  };
  const remove = async () => {
    if (!removing) return;
    setBusy(true);
    try {
      await api.deleteTarget(removing.id);
      setRemoving(null);
      targets.reload();
    } catch (e) {
      toasts.push({ tone: "danger", title: "Could not delete", body: describeHttpError(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card
      title="Targets"
      description="Where notifications go. E-mail needs SUPERHET_SMTP_* on the worker; browser push needs people to turn it on in their account menu."
      actions={
        <Button
          variant="primary"
          size="sm"
          icon={<PlusIcon size={14} />}
          onClick={() => setEditing("new")}
        >
          New target
        </Button>
      }
    >
      <LoadState loading={targets.loading} error={targets.error} onRetry={targets.reload} />
      {targets.data && targets.data.length === 0 && (
        <div className="p-4">
          <EmptyState title="No targets yet">
            Add a webhook (ntfy, Gotify, Slack ...), an e-mail address or browser push.
          </EmptyState>
        </div>
      )}
      {targets.data && targets.data.length > 0 && (
        <ul className="divide-y divide-border" aria-label="Targets">
          {targets.data.map((t) => (
            <li
              key={t.id}
              className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 py-3"
              data-testid={`target-${t.id}`}
            >
              <div className="min-w-0 flex-1 basis-56">
                <div className="flex flex-wrap items-center gap-2 text-sm font-medium">
                  {t.name}
                  <Badge>{t.kind}</Badge>
                  {t.hasSecret && <Badge tone="accent">secret set</Badge>}
                </div>
                <p className="truncate text-xs text-fg-muted">
                  {t.kind === "webhook"
                    ? String(t.config.url_host ?? "")
                    : t.kind === "email"
                      ? Array.isArray(t.config.to)
                        ? (t.config.to as string[]).join(", ")
                        : ""
                      : "accounts with browser notifications on"}
                </p>
              </div>
              <Button size="sm" onClick={() => void test(t)}>
                Send test
              </Button>
              <IconButton label={`Edit ${t.name}`} size="sm" onClick={() => setEditing(t)}>
                <PencilIcon />
              </IconButton>
              <IconButton label={`Delete ${t.name}`} size="sm" onClick={() => setRemoving(t)}>
                <TrashIcon />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
      {editing && (
        <TargetEditor
          target={editing === "new" ? null : editing}
          api={api}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            targets.reload();
          }}
        />
      )}
      {removing && (
        <ConfirmDialog
          title={`Delete ${removing.name}?`}
          confirmLabel="Delete"
          confirmVariant="danger"
          busy={busy}
          onConfirm={() => void remove()}
          onCancel={() => setRemoving(null)}
        >
          Rules that send here lose this target.
        </ConfirmDialog>
      )}
    </Card>
  );
}

function LogTab({ api }: { api: NotificationsApi }) {
  const [status, setStatus] = useState<"" | DeliveryStatus>("");
  const [extra, setExtra] = useState<Delivery[]>([]);
  const [more, setMore] = useState<number | null | undefined>(undefined);
  const load = useCallback(
    (s: AbortSignal) => api.listDeliveries({ status: status || undefined }, s),
    [api, status],
  );
  const first = useLoad(load, 5000);
  const rows = [...(first.data?.deliveries ?? []), ...extra];
  const next = more === undefined ? (first.data?.nextBefore ?? null) : more;

  const loadMore = async () => {
    if (next === null) return;
    try {
      const page = await api.listDeliveries({ status: status || undefined, before: next });
      setExtra((e) => [...e, ...page.deliveries]);
      setMore(page.nextBefore);
    } catch (e) {
      toasts.push({ tone: "danger", title: "Could not load more", body: describeHttpError(e) });
    }
  };

  return (
    <Card
      title="Delivery log"
      description="Every message queued for a target, with its status and the last error. Failed sends are retried with backoff before they are marked failed."
      actions={
        <div className="flex items-end gap-2">
          <Select
            aria-label="Filter by status"
            size="sm"
            value={status}
            onChange={(v) => {
              setStatus(v as "" | DeliveryStatus);
              setExtra([]);
              setMore(undefined);
            }}
            options={[
              { value: "", label: "All" },
              { value: "pending", label: "Pending" },
              { value: "sent", label: "Sent" },
              { value: "failed", label: "Failed" },
            ]}
          />
          <Button size="sm" icon={<RefreshIcon size={14} />} onClick={first.reload}>
            Refresh
          </Button>
        </div>
      }
    >
      <LoadState loading={first.loading} error={first.error} onRetry={first.reload} />
      {first.data && rows.length === 0 && (
        <div className="p-4">
          <EmptyState title="Nothing sent yet">Use Send test on a target or rule.</EmptyState>
        </div>
      )}
      {rows.length > 0 && (
        <ul className="divide-y divide-border" aria-label="Deliveries">
          {rows.map((d) => (
            <li
              key={d.id}
              className="flex flex-wrap items-start gap-x-3 gap-y-1 px-4 py-2.5"
              data-testid={`delivery-${d.id}`}
            >
              <Badge tone={STATUS_TONE[d.status]}>{d.status}</Badge>
              <div className="min-w-0 flex-1 basis-56">
                <p className="truncate text-sm">
                  <span className="font-medium">{d.ruleName}</span> → {d.targetName}
                </p>
                <p className="truncate text-xs text-fg-muted">{d.title}</p>
                {d.lastError && <p className="text-xs text-danger">{d.lastError}</p>}
              </div>
              <span className="text-xs text-fg-subtle num">
                {d.attempts} attempt{d.attempts === 1 ? "" : "s"} · {formatRelative(d.createdAtMs)}
              </span>
            </li>
          ))}
        </ul>
      )}
      {next !== null && (
        <div className="border-t border-border p-3">
          <Button size="sm" onClick={() => void loadMore()}>
            Load older
          </Button>
        </div>
      )}
    </Card>
  );
}
