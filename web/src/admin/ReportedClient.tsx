import { formatDateTime } from "../format";
import { Badge } from "../ui";
import { AddrBadge } from "./ClientInfo";
import {
  compareAddresses,
  describeReportedDevice,
  reportRows,
  type ReportedClient,
} from "./clientReport";

/** Reminder printed wherever client-reported data shows up. */
export const UNVERIFIED_HINT =
  "Reported by the browser itself and not verified: anybody can fake it. Never used for bans, limits or access.";

export function UnverifiedBadge() {
  return (
    <Badge tone="warn" title={UNVERIFIED_HINT}>
      client-reported · unverified
    </Badge>
  );
}

const IP_STATUS_TEXT = {
  blocked: "the browser could not reach the lookup service (blocked or offline)",
  failed: "the lookup service gave no usable answer",
} as const;

/** One line for the session list: what the device claims to be, marked as unverified. */
export function ReportedSummary({ reported }: { reported: ReportedClient }) {
  const text = describeReportedDevice(reported);
  return (
    <span className="inline-flex flex-wrap items-center gap-1.5">
      <span>{text || "Device details reported"}</span>
      <UnverifiedBadge />
    </span>
  );
}

/** The claimed public address next to the address the server sees. */
function PublicIpRows({ reported, seenIp }: { reported: ReportedClient; seenIp: string | null }) {
  const verdict = compareAddresses(reported.publicIp, seenIp);
  if (!reported.publicIp && !reported.publicIpStatus) return null;
  return (
    <>
      <dt className="text-fg-muted">Public IP (client-reported)</dt>
      <dd className="flex flex-wrap items-center gap-1.5">
        {reported.publicIp ? (
          <>
            <span className="font-mono">{reported.publicIp}</span>
            <AddrBadge info={reported.publicIpInfo} />
            {verdict === "differs" && (
              <span className="text-fg-subtle">differs from the address the server sees</span>
            )}
            {verdict === "same" && (
              <span className="text-fg-subtle">same as the address the server sees</span>
            )}
          </>
        ) : (
          <span className="text-fg-subtle">
            not available
            {reported.publicIpStatus && reported.publicIpStatus !== "ok"
              ? `: ${IP_STATUS_TEXT[reported.publicIpStatus]}`
              : ""}
          </span>
        )}
      </dd>
    </>
  );
}

/**
 * Definition-list rows (for a `<dl>` with `sm:grid-cols-[auto_1fr]`) with everything the browser
 * reported, headed by the "client-reported · unverified" mark.
 */
export function ReportedRows({
  reported,
  seenIp,
}: {
  reported: ReportedClient;
  /** The address the server sees for this session or event, as shown. */
  seenIp: string | null;
}) {
  const rows = reportRows(reported);
  return (
    <>
      <dt className="text-fg-muted">Device</dt>
      <dd className="flex flex-wrap items-center gap-1.5">
        <UnverifiedBadge />
        {reported.reportedAtMs !== null && (
          <span className="text-fg-subtle">reported {formatDateTime(reported.reportedAtMs)}</span>
        )}
      </dd>
      {rows.map((r) => (
        <ReportedRow key={r.label} label={r.label} value={r.value} />
      ))}
      <PublicIpRows reported={reported} seenIp={seenIp} />
    </>
  );
}

function ReportedRow({ label, value }: { label: string; value: string }) {
  return (
    <>
      <dt className="text-fg-muted">{label}</dt>
      <dd className="break-words">{value}</dd>
    </>
  );
}

/** Collapsible details block for a session row. */
export function ReportedDetails({
  reported,
  seenIp,
}: {
  reported: ReportedClient;
  seenIp: string | null;
}) {
  return (
    <details className="group rounded-md border border-border bg-surface-2/50 px-3 py-2 text-xs">
      <summary className="focus-ring flex cursor-pointer list-none flex-wrap items-center gap-x-2 gap-y-1 rounded-sm text-fg-muted">
        <ReportedSummary reported={reported} />
        {reported.publicIp && (
          <span>
            · public IP <span className="font-mono text-fg">{reported.publicIp}</span>
          </span>
        )}
      </summary>
      <dl className="mt-2 grid gap-x-6 gap-y-1 sm:grid-cols-[auto_1fr]">
        <ReportedRows reported={reported} seenIp={seenIp} />
      </dl>
      <p className="mt-2 text-fg-subtle">{UNVERIFIED_HINT}</p>
    </details>
  );
}
