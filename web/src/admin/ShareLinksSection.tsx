import { useCallback, useState } from "react";
import type { DeviceInfo, DeviceListApi } from "../devices/api";
import { describeHttpError } from "../http";
import { embedCode, receiverUrl } from "../share/paths";
import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  Dialog,
  Select,
  TextField,
  Toggle,
  toasts,
} from "../ui";
import {
  SHARE_FEATURES,
  type AdminApi,
  type ShareFeature,
  type ShareLink,
  type ShareLinkInput,
} from "./api";
import { Card, LoadState, SectionHeader, TextArea, copyText } from "./common";
import { useLoad } from "./useLoad";

const FEATURE_LABEL: Record<ShareFeature, string> = {
  waterfall: "Waterfall",
  audio: "Audio",
  decoder_events: "Decoder events",
  plugin_audio: "Plugin audio",
};

const MODES = ["wfm", "nfm", "am", "usb", "lsb", "cw"].map((m) => ({
  value: m,
  label: m.toUpperCase(),
}));

const EXPIRY = [
  { value: "keep", label: "Keep as is" },
  { value: "never", label: "Never" },
  { value: "1", label: "In 1 day" },
  { value: "7", label: "In 7 days" },
  { value: "30", label: "In 30 days" },
];

const DAY_MS = 86_400_000;

const mhz = (hz: number | null): string => (hz === null ? "" : String(hz / 1e6));
const toHz = (text: string): number | null => {
  const v = Number(text.replace(",", "."));
  return text.trim() !== "" && Number.isFinite(v) && v > 0 ? Math.round(v * 1e6) : null;
};

export interface Draft {
  label: string;
  title: string;
  description: string;
  location: string;
  device: string;
  tuning: "follow" | "fixed";
  freq: string;
  mode: string;
  min: string;
  max: string;
  features: ShareFeature[];
  maxListeners: string;
  origins: string;
  expiry: string;
}

function draftOf(link: ShareLink | null, device: string): Draft {
  return {
    label: link?.label ?? "",
    title: link?.title ?? "",
    description: link?.description ?? "",
    location: link?.location ?? "",
    device: link?.device ?? device,
    tuning: link?.tuning ?? "follow",
    freq: mhz(link?.freqHz ?? null),
    mode: link?.mode ?? "nfm",
    min: mhz(link?.tuneMinHz ?? null),
    max: mhz(link?.tuneMaxHz ?? null),
    features: link?.features ?? ["waterfall", "audio"],
    maxListeners: String(link?.maxListeners ?? 5),
    origins: (link?.embedOrigins ?? []).join("\n"),
    expiry: link ? "keep" : "never",
  };
}

/** The request body for a draft, or the first thing wrong with it. */
export function inputOf(d: Draft, existing: ShareLink | null): ShareLinkInput | string {
  if (d.label.trim() === "") return "Give the link a name.";
  if (d.features.length === 0) return "Allow at least one feature.";
  const listeners = Number(d.maxListeners);
  if (!Number.isInteger(listeners) || listeners < 1 || listeners > 1000) {
    return "Listeners: a whole number from 1 to 1000.";
  }
  let freqHz: number | null = null;
  let min: number | null = null;
  let max: number | null = null;
  if (d.tuning === "fixed") {
    freqHz = toHz(d.freq);
    if (freqHz === null) return "Fixed links need a frequency in MHz.";
    if (d.min.trim() !== "" || d.max.trim() !== "") {
      min = toHz(d.min);
      max = toHz(d.max);
      if (min === null || max === null || min > max) return "Tuning range: give both ends in MHz.";
      if (freqHz < min || freqHz > max) return "The frequency must lie inside the tuning range.";
    }
  }
  let expiresAtMs: number | null = existing?.expiresAtMs ?? null;
  if (d.expiry === "never") expiresAtMs = null;
  else if (d.expiry !== "keep") expiresAtMs = Date.now() + Number(d.expiry) * DAY_MS;
  return {
    label: d.label.trim(),
    title: d.title.trim(),
    description: d.description.trim(),
    location: d.location.trim(),
    device: d.device,
    tuning: d.tuning,
    freqHz,
    mode: d.tuning === "fixed" ? d.mode : null,
    tuneMinHz: min,
    tuneMaxHz: max,
    features: SHARE_FEATURES.filter((f) => d.features.includes(f)),
    maxListeners: listeners,
    embedOrigins: d.origins
      .split(/[\s,]+/)
      .map((o) => o.trim())
      .filter((o) => o !== ""),
    expiresAtMs,
  };
}

const STATUS_TONE = { active: "success", expired: "neutral", revoked: "danger" } as const;

/** What the admin can do with a link right after it was made: copy it, once. */
function RevealDialog({
  link,
  token,
  onClose,
}: {
  link: ShareLink;
  token: string;
  onClose: () => void;
}) {
  const url = receiverUrl(token);
  const embeddable = link.embedOrigins.length > 0;
  return (
    <Dialog title="Share link ready" size="lg" onClose={onClose}>
      <div className="flex flex-col gap-4 p-4">
        <Alert tone="warn">
          Copy it now. Only a hash is stored, so this link cannot be shown again. If you lose it,
          rotate the token to get a new one.
        </Alert>
        <TextField label="Link for visitors" readOnly value={url} />
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" onClick={() => void copyText(url, "Link")}>
            Copy link
          </Button>
          <Button
            onClick={() => void copyText(embedCode(token), "Embed code")}
            disabled={!embeddable}
            title={embeddable ? undefined : "Add an allowed website to make the player embeddable"}
          >
            Copy embed code
          </Button>
        </div>
        {embeddable && (
          <TextArea
            label="Embed code"
            rows={3}
            value={embedCode(token)}
            onChange={() => undefined}
          />
        )}
        <div className="flex justify-end">
          <Button onClick={onClose}>Done</Button>
        </div>
      </div>
    </Dialog>
  );
}

function Editor({
  existing,
  devices,
  api,
  onClose,
  onSaved,
}: {
  existing: ShareLink | null;
  devices: readonly DeviceInfo[];
  api: AdminApi;
  onClose: () => void;
  onSaved: (made: { link: ShareLink; token: string } | null) => void;
}) {
  const [d, setD] = useState<Draft>(() => draftOf(existing, devices[0]?.id ?? "dev0"));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (patch: Partial<Draft>) => setD({ ...d, ...patch });
  const deviceOptions = [...new Set([...devices.map((x) => x.id), d.device])].map((id) => ({
    value: id,
    label: devices.find((x) => x.id === id)?.label ?? id,
  }));

  const save = async () => {
    const input = inputOf(d, existing);
    if (typeof input === "string") {
      setError(input);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (existing) {
        await api.updateShareLink(existing.id, input);
        onSaved(null);
      } else {
        onSaved(await api.createShareLink(input));
      }
    } catch (e) {
      setError(describeHttpError(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog title={existing ? "Edit share link" : "New share link"} size="lg" onClose={onClose}>
      <div className="flex max-h-[70vh] flex-col gap-4 overflow-y-auto p-4">
        <TextField
          label="Name (only you see it)"
          value={d.label}
          onChange={(e) => set({ label: e.target.value })}
          maxLength={80}
        />
        <div className="grid gap-4 sm:grid-cols-2">
          <TextField
            label="Station name"
            value={d.title}
            onChange={(e) => set({ title: e.target.value })}
            maxLength={120}
          />
          <TextField
            label="Location"
            value={d.location}
            onChange={(e) => set({ location: e.target.value })}
            maxLength={200}
          />
        </div>
        <TextArea
          label="Description"
          rows={3}
          maxLength={1000}
          value={d.description}
          onChange={(description) => set({ description })}
        />
        <div className="grid gap-4 sm:grid-cols-2">
          <Select
            label="Receiver"
            options={deviceOptions}
            value={d.device}
            onChange={(device) => set({ device })}
          />
          <Select
            label="Tuned to"
            options={[
              { value: "follow", label: "Follow the operator" },
              { value: "fixed", label: "A fixed frequency" },
            ]}
            value={d.tuning}
            onChange={(tuning) => set({ tuning: tuning === "fixed" ? "fixed" : "follow" })}
          />
        </div>
        {d.tuning === "fixed" && (
          <div className="grid gap-4 sm:grid-cols-2">
            <TextField
              label="Frequency (MHz)"
              inputMode="decimal"
              value={d.freq}
              onChange={(e) => set({ freq: e.target.value })}
            />
            <Select
              label="Mode"
              options={MODES}
              value={d.mode}
              onChange={(mode) => set({ mode })}
            />
            <TextField
              label="Visitors may tune from (MHz)"
              inputMode="decimal"
              value={d.min}
              onChange={(e) => set({ min: e.target.value })}
              hint="Leave both empty for no tuning."
            />
            <TextField
              label="up to (MHz)"
              inputMode="decimal"
              value={d.max}
              onChange={(e) => set({ max: e.target.value })}
            />
          </div>
        )}
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1 text-xs font-medium text-fg-muted">Visitors may use</legend>
          {SHARE_FEATURES.map((f) => (
            <Toggle
              key={f}
              label={FEATURE_LABEL[f]}
              checked={d.features.includes(f)}
              onChange={(on) =>
                set({ features: on ? [...d.features, f] : d.features.filter((x) => x !== f) })
              }
            />
          ))}
        </fieldset>
        <div className="grid gap-4 sm:grid-cols-2">
          <TextField
            label="Listeners at most"
            inputMode="numeric"
            value={d.maxListeners}
            onChange={(e) => set({ maxListeners: e.target.value })}
          />
          <Select
            label="Expires"
            options={existing ? EXPIRY : EXPIRY.filter((o) => o.value !== "keep")}
            value={d.expiry}
            onChange={(expiry) => set({ expiry })}
          />
        </div>
        <TextArea
          label="Websites allowed to embed the player"
          rows={3}
          value={d.origins}
          onChange={(origins) => set({ origins })}
          hint="One origin per line, like https://blog.example.org. Empty: the player cannot be embedded."
        />
        {error && <Alert tone="danger">{error}</Alert>}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void save()} disabled={busy}>
            {existing ? "Save" : "Create link"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

function when(ms: number | null): string {
  return ms === null ? "never" : new Date(ms).toLocaleString();
}

type Modal =
  | { kind: "edit"; link: ShareLink | null }
  | { kind: "reveal"; link: ShareLink; token: string }
  | { kind: "revoke"; link: ShareLink }
  | { kind: "rotate"; link: ShareLink }
  | { kind: "delete"; link: ShareLink };

export function ShareLinksSection({ api, deviceApi }: { api: AdminApi; deviceApi: DeviceListApi }) {
  const load = useCallback(
    async (signal: AbortSignal) => {
      const [links, devices] = await Promise.all([
        api.getShareLinks(signal),
        deviceApi.list(signal).catch(() => [] as DeviceInfo[]),
      ]);
      return { links, devices };
    },
    [api, deviceApi],
  );
  const loaded = useLoad(load, 10_000);
  const [modal, setModal] = useState<Modal | null>(null);
  const [busy, setBusy] = useState(false);
  const data = loaded.data;

  const act = async (action: () => Promise<Modal | null>, done: string) => {
    setBusy(true);
    try {
      const next = await action();
      toasts.push({ tone: "success", title: done });
      setModal(next);
      loaded.reload();
    } catch (e) {
      toasts.push({ tone: "danger", title: "Not done", body: describeHttpError(e) });
      setModal(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Share links"
        description="Public receiver pages for people without an account. A link opens a clean page (and an embeddable player) with exactly what you allow here; revoke it any time."
        actions={
          <Button variant="primary" onClick={() => setModal({ kind: "edit", link: null })}>
            New link
          </Button>
        }
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {data && data.links.length === 0 && (
        <p className="text-sm text-fg-muted">No share links yet.</p>
      )}
      {data && data.links.length > 0 && (
        <ul aria-label="Share links" className="flex flex-col gap-4">
          {data.links.map((link) => (
            <li key={link.id}>
              <Card
                title={link.label}
                description={`${link.title || "No station name"} · ${link.device} · …${link.tokenHint}`}
                actions={<Badge tone={STATUS_TONE[link.status]}>{link.status}</Badge>}
              >
                <div data-testid={`link-${link.id}`} className="flex flex-col gap-3 p-4 text-sm">
                  <p className="text-fg-muted">
                    {link.tuning === "fixed" && link.freqHz !== null
                      ? `${link.freqHz / 1e6} MHz ${link.mode?.toUpperCase() ?? ""}`
                      : "Follows the operator"}
                    {link.tuneMinHz !== null && link.tuneMaxHz !== null
                      ? `, tunable ${link.tuneMinHz / 1e6} to ${link.tuneMaxHz / 1e6} MHz`
                      : ""}
                    {" · "}
                    {link.features.map((f) => FEATURE_LABEL[f]).join(", ")}
                    {" · "}
                    {link.listeners} of {link.maxListeners} listeners
                    {" · "}
                    {link.embedOrigins.length > 0
                      ? `embeddable on ${link.embedOrigins.join(", ")}`
                      : "not embeddable"}
                    {" · expires "}
                    {when(link.expiresAtMs)}
                  </p>
                  <div className="flex flex-wrap justify-end gap-2">
                    <Button onClick={() => setModal({ kind: "edit", link })}>Edit</Button>
                    <Button onClick={() => setModal({ kind: "rotate", link })}>New token</Button>
                    {link.status !== "revoked" && (
                      <Button onClick={() => setModal({ kind: "revoke", link })}>Revoke</Button>
                    )}
                    <Button variant="danger" onClick={() => setModal({ kind: "delete", link })}>
                      Delete
                    </Button>
                  </div>
                </div>
              </Card>
            </li>
          ))}
        </ul>
      )}
      {modal?.kind === "edit" && data && (
        <Editor
          existing={modal.link}
          devices={data.devices}
          api={api}
          onClose={() => setModal(null)}
          onSaved={(made) => {
            loaded.reload();
            setModal(made ? { kind: "reveal", ...made } : null);
          }}
        />
      )}
      {modal?.kind === "reveal" && (
        <RevealDialog link={modal.link} token={modal.token} onClose={() => setModal(null)} />
      )}
      {modal?.kind === "revoke" && (
        <ConfirmDialog
          title="Revoke this link?"
          confirmLabel="Revoke"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setModal(null)}
          onConfirm={() =>
            void act(async () => {
              await api.revokeShareLink(modal.link.id);
              return null;
            }, "Link revoked")
          }
        >
          Everybody listening through “{modal.link.label}” is disconnected and the link stops
          working.
        </ConfirmDialog>
      )}
      {modal?.kind === "rotate" && (
        <ConfirmDialog
          title="Make a new token?"
          confirmLabel="New token"
          busy={busy}
          onCancel={() => setModal(null)}
          onConfirm={() =>
            void act(async () => {
              const made = await api.rotateShareLink(modal.link.id);
              return { kind: "reveal", ...made };
            }, "Token replaced")
          }
        >
          The old link stops working at once (and a revoked link works again with the new one).
        </ConfirmDialog>
      )}
      {modal?.kind === "delete" && (
        <ConfirmDialog
          title="Delete this link?"
          confirmLabel="Delete"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setModal(null)}
          onConfirm={() =>
            void act(async () => {
              await api.deleteShareLink(modal.link.id);
              return null;
            }, "Link deleted")
          }
        >
          “{modal.link.label}” is removed for good.
        </ConfirmDialog>
      )}
    </div>
  );
}
