import { useCallback, useState } from "react";
import type { Role } from "../auth/api";
import type { DeviceInfo, DeviceListApi } from "../devices/api";
import { describeHttpError } from "../http";
import { Alert, Badge, Button, Chip, EmptyState, Select, Toggle, toasts } from "../ui";
import { openPolicy, type AdminApi, type DevicePolicy, type User } from "./api";
import { Card, LoadState, SectionHeader } from "./common";
import { useLoad } from "./useLoad";

const VIEW_OPTIONS = [
  { value: "listener", label: "Everyone signed in" },
  { value: "operator", label: "Operators and admins" },
  { value: "admin", label: "Admins only" },
];

const CONTROL_OPTIONS = [
  { value: "listener", label: "Everyone who can see it" },
  { value: "operator", label: "Operators and admins" },
  { value: "admin", label: "Admins only" },
];

/** True when the policy is today's open behaviour. */
export function isOpen(p: DevicePolicy): boolean {
  return (
    p.viewRole === "listener" &&
    p.controlRole === "listener" &&
    p.guestsVisible &&
    p.allowedUsers.length === 0
  );
}

/** One line that says who gets what on a device. */
export function summary(p: DevicePolicy): string {
  if (isOpen(p)) return "Open to everyone";
  const parts: string[] = [];
  if (p.allowedUsers.length > 0) {
    parts.push(
      `${p.allowedUsers.length} selected ${p.allowedUsers.length === 1 ? "account" : "accounts"} and admins only`,
    );
  } else {
    if (p.viewRole !== "listener") parts.push(`seen by ${p.viewRole}s and up`);
    if (!p.guestsVisible && p.viewRole === "listener") parts.push("hidden from guests");
  }
  if (p.controlRole !== "listener") parts.push(`changed by ${p.controlRole}s and up`);
  return parts.join(", ");
}

interface DeviceRow {
  id: string;
  label: string | null;
  online: boolean | null;
}

function rowsOf(devices: readonly DeviceInfo[], policies: readonly DevicePolicy[]): DeviceRow[] {
  const rows = new Map<string, DeviceRow>();
  for (const d of devices) rows.set(d.id, { id: d.id, label: d.label, online: d.online });
  // A policy can outlive its daemon: keep it editable.
  for (const p of policies) {
    if (!rows.has(p.device)) rows.set(p.device, { id: p.device, label: null, online: null });
  }
  return [...rows.values()].sort((a, b) => a.id.localeCompare(b.id));
}

interface Data {
  policies: DevicePolicy[];
  users: User[];
  devices: DeviceInfo[];
}

export function DevicePoliciesSection({
  api,
  deviceApi,
}: {
  api: AdminApi;
  deviceApi: DeviceListApi;
}) {
  const load = useCallback(
    async (signal: AbortSignal): Promise<Data> => {
      const [policies, users, devices] = await Promise.all([
        api.getDevicePolicies(signal),
        api.listUsers(signal),
        // The list is a convenience: a bus outage must not hide the stored policies.
        deviceApi.list(signal).catch(() => [] as DeviceInfo[]),
      ]);
      return { policies, users, devices };
    },
    [api, deviceApi],
  );
  const loaded = useLoad(load);
  const data = loaded.data;
  const rows = data ? rowsOf(data.devices, data.policies) : [];

  return (
    <div>
      <SectionHeader
        title="Device access"
        description="Decide who may see and who may change each SDR. Without a policy a device is open to everyone signed in (and to guests, while guest access is on). Admins always have full access."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {data && rows.length === 0 && (
        <EmptyState title="No devices known">
          Devices appear here once their daemon has run. Start a device service first.
        </EmptyState>
      )}
      {data && (
        <ul aria-label="Devices" className="flex flex-col gap-6">
          {rows.map((row) => {
            const saved = data.policies.find((p) => p.device === row.id) ?? openPolicy(row.id);
            return (
              <li key={`${row.id}:${JSON.stringify(saved)}`}>
                <DeviceCard
                  row={row}
                  saved={saved}
                  users={data.users}
                  api={api}
                  onSaved={loaded.reload}
                />
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function DeviceCard({
  row,
  saved,
  users,
  api,
  onSaved,
}: {
  row: DeviceRow;
  saved: DevicePolicy;
  users: readonly User[];
  api: AdminApi;
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState<DevicePolicy>(saved);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const restricted = draft.allowedUsers.length > 0;
  // Admins pass everywhere; listing them would only suggest they needed it.
  const accounts = users.filter((u) => u.role !== "admin" && !u.disabled);
  const [listing, setListing] = useState(restricted);
  const set = (patch: Partial<DevicePolicy>) => setDraft({ ...draft, ...patch });
  const name = row.label ?? row.id;

  const run = async (action: () => Promise<string | undefined>) => {
    setBusy(true);
    setError(null);
    try {
      const note = await action();
      toasts.push({ tone: "success", title: `Access to ${name} saved`, body: note });
      onSaved();
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  const save = () =>
    run(async () => {
      const { endedSessions } = await api.putDevicePolicy(draft);
      return endedSessions > 0
        ? `${endedSessions} ${endedSessions === 1 ? "session" : "sessions"} that lost access were disconnected.`
        : undefined;
    });
  const reset = () =>
    run(async () => {
      await api.resetDevicePolicy(row.id);
      return "The device is open again.";
    });

  const toggleAccount = (id: number) =>
    set({
      allowedUsers: draft.allowedUsers.includes(id)
        ? draft.allowedUsers.filter((u) => u !== id)
        : [...draft.allowedUsers, id].sort((a, b) => a - b),
    });

  return (
    <Card
      title={name}
      description={row.label ? row.id : undefined}
      actions={
        <span className="flex items-center gap-2">
          {row.online === false && <Badge tone="neutral">Offline</Badge>}
          <Badge tone={isOpen(saved) ? "neutral" : "accent"}>{summary(saved)}</Badge>
        </span>
      }
    >
      <div data-testid={`device-${row.id}`} className="flex flex-col gap-4 p-4">
        <div className="grid gap-4 sm:grid-cols-2">
          <Select
            label="Who can see and listen"
            options={VIEW_OPTIONS}
            value={draft.viewRole}
            disabled={restricted}
            onChange={(v) => set({ viewRole: v as Role })}
          />
          <Select
            label="Who can change it"
            options={CONTROL_OPTIONS}
            value={draft.controlRole}
            onChange={(v) => set({ controlRole: v as Role })}
          />
        </div>
        <p className="text-xs text-fg-subtle">
          Changing means tuning, opening receivers, the tuning lock, profiles and plugin decoders.
          People who can see a device but not change it get a read-only view.
        </p>
        <Toggle
          label="Guests can see it"
          checked={draft.guestsVisible && !restricted}
          disabled={restricted}
          onChange={(guestsVisible) => set({ guestsVisible })}
        />
        <div className="flex flex-col gap-2">
          <Toggle
            label="Only selected accounts"
            checked={listing}
            onChange={(on) => {
              setListing(on);
              if (!on) set({ allowedUsers: [] });
            }}
          />
          {listing && (
            <>
              <p className="text-xs text-fg-subtle">
                Only these accounts (and admins) can use the device, no guests. Their role still
                decides what they may change.
              </p>
              {accounts.length === 0 ? (
                <p className="text-xs text-fg-muted">There are no operator or listener accounts.</p>
              ) : (
                <ul aria-label={`Accounts allowed on ${name}`} className="flex flex-wrap gap-2">
                  {accounts.map((u) => (
                    <li key={u.id}>
                      <Chip
                        selected={draft.allowedUsers.includes(u.id)}
                        onClick={() => toggleAccount(u.id)}
                      >
                        {u.username}
                      </Chip>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
        {error && <Alert>{error}</Alert>}
        <div className="flex items-center justify-end gap-2">
          <span className="mr-auto text-xs text-fg-subtle">
            {dirty ? "Unsaved changes" : "All changes saved"}
          </span>
          {!isOpen(saved) && (
            <Button onClick={() => void reset()} disabled={busy}>
              Reset to open
            </Button>
          )}
          <Button
            variant="primary"
            onClick={() => void save()}
            disabled={!dirty || busy || (listing && !restricted)}
            title={
              listing && !restricted
                ? "Pick at least one account, or switch the list off"
                : undefined
            }
          >
            {busy ? "Saving…" : "Save changes"}
          </Button>
        </div>
      </div>
    </Card>
  );
}
