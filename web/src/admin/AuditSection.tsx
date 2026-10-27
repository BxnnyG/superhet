import { useCallback, useMemo, useState } from "react";
import { SHELL_QUERY, useMediaQuery } from "../shell/useMediaQuery";
import { ROLES } from "../auth/api";
import { ROLE_LABEL } from "../auth/permissions";
import { formatDateTime } from "../format";
import {
  Badge,
  Button,
  ChevronDownIcon,
  ChevronRightIcon,
  DownloadIcon,
  EmptyState,
  PlainButton,
  RefreshIcon,
  Select,
  TextField,
  cn,
  type BadgeTone,
} from "../ui";
import { BUTTON_SIZE, BUTTON_VARIANT } from "../ui/Button";
import type { AdminApi, AuditEvent, AuditFilter, Outcome } from "./api";
import {
  EMPTY_FILTER,
  RANGE_OPTIONS,
  activeFilters,
  filterProblem,
  toFilter,
  type FilterForm,
} from "./auditFilter";
import { BanDialog } from "./BanDialog";
import { ClientRows, GatewayBanner } from "./ClientInfo";
import { Card, LoadState, RoleBadge, SectionHeader } from "./common";
import { useLoad } from "./useLoad";

const OUTCOME_TONE: Record<Outcome, BadgeTone> = { ok: "success", denied: "warn", error: "danger" };

const PAGE_SIZES = [
  { value: "50", label: "50 rows" },
  { value: "100", label: "100 rows" },
  { value: "250", label: "250 rows" },
];

export function AuditSection({ api }: { api: AdminApi }) {
  const [form, setForm] = useState<FilterForm>(EMPTY_FILTER);
  // The filter the table shows; changed by Apply / Reset / Refresh so typing does not query.
  const [applied, setApplied] = useState<AuditFilter>(() => toFilter(EMPTY_FILTER));
  const [pageSize, setPageSize] = useState("100");
  // Cursor stack: the first page has no cursor, every further page the `before_id` of the last.
  const [cursors, setCursors] = useState<(number | undefined)[]>([undefined]);
  const [expanded, setExpanded] = useState<number | null>(null);
  const [banning, setBanning] = useState<AuditEvent | null>(null);
  // Wide screens show the filter fields; on a phone they fold away behind a "Filters" button.
  const wide = useMediaQuery(SHELL_QUERY);
  const [fieldsOpen, setFieldsOpen] = useState<boolean | null>(null);
  const showFields = fieldsOpen ?? wide;
  const problem = filterProblem(form);
  const cursor = cursors[cursors.length - 1];

  const load = useCallback(
    (signal: AbortSignal) =>
      api.audit(applied, { limit: Number(pageSize), beforeId: cursor }, signal),
    [api, applied, pageSize, cursor],
  );
  const page = useLoad(load);

  const apply = () => {
    if (problem) return;
    setApplied(toFilter(form));
    setCursors([undefined]);
    setExpanded(null);
  };
  const reset = () => {
    setForm(EMPTY_FILTER);
    setApplied(toFilter(EMPTY_FILTER));
    setCursors([undefined]);
    setExpanded(null);
  };
  const set = (patch: Partial<FilterForm>) => setForm((f) => ({ ...f, ...patch }));
  const n = activeFilters(form);

  const csv = useMemo(() => api.exportUrl(applied, "csv"), [api, applied]);
  const json = useMemo(() => api.exportUrl(applied, "json"), [api, applied]);
  const events = page.data?.events ?? [];
  const next = page.data?.nextBeforeId ?? null;

  return (
    <div>
      <SectionHeader
        title="Audit log"
        description="Who did what, newest first. Entries cannot be changed; retention and address storage are set under Audit settings."
        actions={
          <>
            <ExportLink href={csv} label="Export CSV" />
            <ExportLink href={json} label="Export JSON" />
          </>
        }
      />
      {events.length > 0 && (
        <div className="mb-4">
          <GatewayBanner infos={events.map((e) => e.client.ipInfo)} />
        </div>
      )}

      <Card className="mb-4">
        <form
          className="flex flex-col gap-4 p-4"
          aria-label="Audit filters"
          onSubmit={(e) => {
            e.preventDefault();
            apply();
          }}
        >
          <div className={showFields ? "grid gap-3 sm:grid-cols-2 lg:grid-cols-4" : "hidden"}>
            <Select
              label="Time range"
              value={form.range}
              onChange={(v) => set({ range: v as FilterForm["range"] })}
              options={RANGE_OPTIONS}
            />
            {form.range === "custom" && (
              <>
                <TextField
                  label="From"
                  type="datetime-local"
                  value={form.from}
                  onChange={(e) => set({ from: e.target.value })}
                />
                <TextField
                  label="To"
                  type="datetime-local"
                  value={form.to}
                  error={problem}
                  onChange={(e) => set({ to: e.target.value })}
                />
              </>
            )}
            <TextField
              label="Actor"
              placeholder="name or id"
              value={form.actor}
              onChange={(e) => set({ actor: e.target.value })}
            />
            <Select
              label="Actor type"
              value={form.actorKind}
              onChange={(v) => set({ actorKind: v as FilterForm["actorKind"] })}
              options={[
                { value: "", label: "Any" },
                { value: "user", label: "Account" },
                { value: "guest", label: "Guest" },
                { value: "system", label: "System" },
              ]}
            />
            <Select
              label="Role"
              value={form.role}
              onChange={(v) => set({ role: v as FilterForm["role"] })}
              options={[
                { value: "", label: "Any" },
                ...ROLES.map((r) => ({ value: r, label: ROLE_LABEL[r] })),
              ]}
            />
            <TextField
              label="Action"
              placeholder="auth.login or user.*"
              autoCapitalize="off"
              spellCheck={false}
              value={form.action}
              onChange={(e) => set({ action: e.target.value })}
            />
            <Select
              label="Outcome"
              value={form.outcome}
              onChange={(v) => set({ outcome: v as FilterForm["outcome"] })}
              options={[
                { value: "", label: "Any" },
                { value: "ok", label: "OK" },
                { value: "denied", label: "Denied" },
                { value: "error", label: "Error" },
              ]}
            />
            <TextField
              label="IP address or network"
              placeholder="198.51.100.7 or 198.51.100.0/24"
              spellCheck={false}
              value={form.ip}
              onChange={(e) => set({ ip: e.target.value })}
            />
            <TextField
              label="Target"
              placeholder="part of the target"
              value={form.target}
              onChange={(e) => set({ target: e.target.value })}
            />
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              aria-expanded={showFields}
              icon={<ChevronDownIcon size={14} className={showFields ? "" : "-rotate-90"} />}
              onClick={() => setFieldsOpen(!showFields)}
              className="shell:hidden"
            >
              Filters{n > 0 ? ` (${n})` : ""}
            </Button>
            <Button type="submit" variant="primary" disabled={problem !== null}>
              Apply filters{n > 0 ? ` (${n})` : ""}
            </Button>
            <Button onClick={reset}>Reset</Button>
            <Button icon={<RefreshIcon size={14} />} onClick={apply} disabled={problem !== null}>
              Refresh
            </Button>
            <div className="ml-auto w-32">
              <Select
                aria-label="Rows per page"
                size="sm"
                value={pageSize}
                onChange={(v) => {
                  setPageSize(v);
                  setCursors([undefined]);
                }}
                options={PAGE_SIZES}
              />
            </div>
          </div>
        </form>
      </Card>

      <Card>
        <LoadState loading={page.loading} error={page.error} onRetry={page.reload} />
        {page.data && events.length === 0 && (
          <div className="p-4">
            <EmptyState title="No matching entries">
              Widen the time range or clear a filter.
            </EmptyState>
          </div>
        )}
        {page.data && events.length > 0 && (
          <div className="overflow-x-auto">
            <table className="w-full min-w-[44rem] text-left text-sm" aria-label="Audit events">
              <thead>
                <tr className="border-b border-border text-xs text-fg-muted">
                  <th scope="col" className="w-8 px-2 py-2">
                    <span className="sr-only">Details</span>
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Time
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Actor
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Action
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Target
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Outcome
                  </th>
                  <th scope="col" className="px-2 py-2 font-medium">
                    Address
                  </th>
                </tr>
              </thead>
              <tbody>
                {events.map((ev) => (
                  <EventRows
                    key={ev.id}
                    ev={ev}
                    open={expanded === ev.id}
                    onToggle={() => setExpanded(expanded === ev.id ? null : ev.id)}
                    onBan={setBanning}
                  />
                ))}
              </tbody>
            </table>
          </div>
        )}
        <div className="flex items-center justify-between gap-2 border-t border-border px-4 py-2.5">
          <span className="text-xs text-fg-muted">
            Page {cursors.length}
            {page.data ? ` · ${events.length} entries` : ""}
          </span>
          <div className="flex gap-2">
            <Button
              size="sm"
              disabled={cursors.length <= 1}
              onClick={() => setCursors((c) => c.slice(0, -1))}
            >
              Newer
            </Button>
            <Button
              size="sm"
              disabled={next === null}
              onClick={() => next !== null && setCursors((c) => [...c, next])}
            >
              Older
            </Button>
          </div>
        </div>
      </Card>
      {banning?.ip && (
        <BanDialog
          api={api}
          target={{
            kind: "network",
            network: banning.ip,
            hint: "The address as stored in the log. With the truncated address mode this is the network address (/24 for IPv4, /48 for IPv6): append the prefix, e.g. /24, to ban the whole network.",
          }}
          onClose={() => setBanning(null)}
          onDone={() => undefined}
        />
      )}
    </div>
  );
}

/** Download link styled like a button (an attachment response; the cookie rides along). */
function ExportLink({ href, label }: { href: string; label: string }) {
  return (
    <a
      href={href}
      download
      className={cn(
        "focus-ring inline-flex shrink-0 items-center justify-center rounded-md font-medium",
        "transition-colors duration-150",
        BUTTON_VARIANT.secondary,
        BUTTON_SIZE.md,
      )}
    >
      <DownloadIcon size={14} />
      {label}
    </a>
  );
}

function EventRows({
  ev,
  open,
  onToggle,
  onBan,
}: {
  ev: AuditEvent;
  open: boolean;
  onToggle: () => void;
  onBan: (ev: AuditEvent) => void;
}) {
  const who = ev.actorName ?? ev.actorId ?? "—";
  return (
    <>
      <tr className="border-b border-border/60 align-top hover:bg-surface-2/60">
        <td className="px-2 py-2">
          <PlainButton
            aria-expanded={open}
            aria-label={`${open ? "Hide" : "Show"} details of event ${ev.id}`}
            onClick={onToggle}
            className="grid size-6 place-items-center rounded text-fg-subtle hover:text-fg"
          >
            <ChevronRightIcon
              size={14}
              className={cn("transition-transform", open && "rotate-90")}
            />
          </PlainButton>
        </td>
        <td className="px-2 py-2 font-mono text-xs whitespace-nowrap text-fg-muted num">
          {formatDateTime(ev.tsMs)}
        </td>
        <td className="px-2 py-2">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="font-medium">{who}</span>
            {ev.actorKind === "system" ? (
              <Badge>System</Badge>
            ) : ev.role ? (
              <RoleBadge role={ev.role} guest={ev.actorKind === "guest"} />
            ) : ev.actorKind === "guest" ? (
              <Badge tone="warn">Guest</Badge>
            ) : null}
          </div>
        </td>
        <td className="px-2 py-2 font-mono text-xs">{ev.action}</td>
        <td className="max-w-56 truncate px-2 py-2 text-xs text-fg-muted" title={ev.target ?? ""}>
          {ev.target ?? "—"}
        </td>
        <td className="px-2 py-2">
          <Badge tone={OUTCOME_TONE[ev.outcome]}>{ev.outcome}</Badge>
        </td>
        <td className="px-2 py-2 font-mono text-xs whitespace-nowrap text-fg-muted">
          {ev.ip ?? "—"}
        </td>
      </tr>
      {open && (
        <tr className="border-b border-border/60 bg-surface-2/50">
          <td />
          <td colSpan={6} className="px-2 py-3">
            <dl className="grid gap-x-6 gap-y-1 text-xs sm:grid-cols-[auto_1fr]">
              <dt className="text-fg-muted">Event</dt>
              <dd className="font-mono">#{ev.id}</dd>
              <ClientRows ip={ev.ip} info={ev.client} userAgent={ev.userAgent} />
              {ev.ip && (
                <>
                  <dt className="text-fg-muted">Ban</dt>
                  <dd>
                    <Button size="sm" variant="danger" onClick={() => onBan(ev)}>
                      Ban {ev.ip}…
                    </Button>
                  </dd>
                </>
              )}
              <dt className="text-fg-muted">Details</dt>
              <dd>
                <pre className="font-mono whitespace-pre-wrap break-all">
                  {Object.keys(ev.details).length === 0 ? "—" : JSON.stringify(ev.details, null, 2)}
                </pre>
              </dd>
            </dl>
          </td>
        </tr>
      )}
    </>
  );
}
