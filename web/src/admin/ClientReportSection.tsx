import { useCallback, useState } from "react";
import { describeHttpError } from "../http";
import { Alert, Select, TextField, Toggle, toasts } from "../ui";
import {
  DEFAULT_PUBLIC_IP_SERVICE,
  type AdminApi,
  type ClientReportPolicy,
  type CollectScope,
} from "./api";
import { CLIENT_ADDR_DOC } from "./ClientInfo";
import { Card, LoadState, SaveBar, SectionHeader } from "./common";
import { useDraft } from "./useDraft";
import { useLoad } from "./useLoad";

const SCOPE_OPTIONS: readonly { value: CollectScope; label: string }[] = [
  { value: "off", label: "Nobody (off)" },
  { value: "users", label: "Signed-in accounts" },
  { value: "all", label: "Accounts and guests" },
];

/** What is wrong with the policy before it is sent (the server validates again). */
export function clientReportProblem(p: ClientReportPolicy): string | null {
  if (!p.askPublicIp || p.collect === "off") return null;
  const url = p.publicIpService.trim();
  if (!url.toLowerCase().startsWith("https://")) {
    return "The lookup service must be an https:// URL.";
  }
  if (/\s/.test(url) || url.includes("@")) {
    return "The lookup service URL must not contain spaces or credentials.";
  }
  return null;
}

export function ClientReportSection({ api }: { api: AdminApi }) {
  const load = useCallback((s: AbortSignal) => api.getClientReportPolicy(s), [api]);
  const loaded = useLoad(load);
  const [saved, setSaved] = useState<ClientReportPolicy | null>(null);
  const draft = useDraft(saved ?? loaded.data);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const p = draft.value;
  const problem = p ? clientReportProblem(p) : null;

  const update = (patch: Partial<ClientReportPolicy>) => p && draft.set({ ...p, ...patch });

  const save = async () => {
    if (!p || problem) return;
    setBusy(true);
    setError(null);
    try {
      setSaved(
        await api.putClientReportPolicy({ ...p, publicIpService: p.publicIpService.trim() }),
      );
      toasts.push({
        tone: "success",
        title: "Client info policy saved",
        body: "Browsers pick it up on their next page load.",
      });
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Client device info"
        description="Let browsers tell you what they are (device, screen, language, time zone) and, optionally, their public IP address. Off by default."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {p && (
        <div className="flex flex-col gap-6">
          <Card
            title="Device details"
            description="Collected once per page load through the browser's own APIs (User-Agent Client Hints, screen, languages, time zone, hardware hints) and shown in Live sessions and the audit log."
          >
            <div className="flex flex-col gap-3 p-4">
              <Select
                label="Ask the browsers of"
                value={p.collect}
                onChange={(collect) => update({ collect: collect as CollectScope })}
                options={SCOPE_OPTIONS}
              />
              <p className="text-xs text-fg-subtle">
                Everything is reported by the browser itself and cannot be verified, so it is always
                shown as <strong>client-reported · unverified</strong>. It is never used for bans,
                rate limits or access decisions. For accounts it is kept with the session; for
                guests it is kept in memory until the guest session ends. The audit log keeps a copy
                under your audit retention, with the public address stored in the address mode of
                that group (see Audit settings).
              </p>
            </div>
          </Card>

          <Card
            title="Public IP address"
            description="The server only sees the address a connection came from. Behind Docker or a LAN that is a private address, not the visitor's public one."
          >
            <div className="flex flex-col gap-4 p-4">
              <Toggle
                label="Ask clients for their public IP"
                checked={p.askPublicIp}
                disabled={p.collect === "off"}
                onChange={(askPublicIp) => update({ askPublicIp })}
              />
              {p.collect === "off" && (
                <p className="-mt-2 text-xs text-fg-subtle">
                  Choose whose browsers to ask above first.
                </p>
              )}
              {p.askPublicIp && p.collect !== "off" && (
                <>
                  <TextField
                    label="Lookup service"
                    value={p.publicIpService}
                    spellCheck={false}
                    autoComplete="off"
                    onChange={(e) => update({ publicIpService: e.target.value })}
                    error={problem}
                    hint={
                      <>
                        An https URL that answers <code>{'{"ip":"…"}'}</code> or just the address.
                        Default: <code>{DEFAULT_PUBLIC_IP_SERVICE}</code>
                      </>
                    }
                  />
                  <Alert tone="warn">
                    <strong>Privacy.</strong> The visitor&apos;s browser connects to that service,
                    so the third party sees their IP address (and that they use Superhet only if the
                    service logs the referrer, which Superhet suppresses). Say so in the guest
                    privacy notice, and pick a service you trust or run your own. The reported
                    address is spoofable; it is for information only, not for bans or limits.{" "}
                    <a
                      className="underline"
                      href={CLIENT_ADDR_DOC}
                      target="_blank"
                      rel="noreferrer noopener"
                    >
                      Details
                    </a>
                  </Alert>
                  {p.collect === "all" && (
                    <p className="text-xs text-fg-subtle">
                      Guests are shown a line about this before they start a session, in addition to
                      your privacy notice.
                    </p>
                  )}
                </>
              )}
            </div>
          </Card>

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
