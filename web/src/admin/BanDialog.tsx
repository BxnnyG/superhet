import { useState, type SyntheticEvent } from "react";
import { describeHttpError } from "../http";
import { Alert, Button, Dialog, Select, TextField, Toggle, toasts } from "../ui";
import type { AdminApi, BanRequest } from "./api";

const DURATIONS = [
  { value: "15", label: "15 minutes" },
  { value: "60", label: "1 hour" },
  { value: "1440", label: "1 day" },
  { value: "10080", label: "7 days" },
  { value: "43200", label: "30 days" },
  { value: "0", label: "Permanent" },
];

/** What the dialog bans. A live session fixes the exact address on the server side. */
export type BanTarget =
  | { kind: "session"; sid: string; label: string }
  | { kind: "network"; network: string; hint?: string };

interface Props {
  api: AdminApi;
  target: BanTarget;
  onClose: () => void;
  /** Called after the ban exists (the caller refreshes its lists). */
  onDone: () => void;
}

/** Shared "Kick and ban" / "Ban an address" dialog. */
export function BanDialog({ api, target, onClose, onDone }: Props) {
  const [network, setNetwork] = useState(target.kind === "network" ? target.network : "");
  const [duration, setDuration] = useState("1440");
  const [reason, setReason] = useState("");
  const [kick, setKick] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState(false);
  const networkMissing = target.kind === "network" && network.trim() === "";

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (busy) return;
    setSubmitted(true);
    setError(null);
    if (networkMissing) return;
    const req: BanRequest = {
      target: target.kind === "session" ? { session: target.sid } : { network: network.trim() },
      kick,
      ...(reason.trim() ? { reason: reason.trim() } : {}),
      ...(duration !== "0" ? { expiresInMinutes: Number(duration) } : {}),
    };
    setBusy(true);
    try {
      const r = await api.createBan(req);
      toasts.push({
        tone: "success",
        title: `${r.ban.network} is banned`,
        ...(r.kicked > 0 ? { body: `${r.kicked} session${r.kicked === 1 ? "" : "s"} ended.` } : {}),
      });
      onDone();
      onClose();
    } catch (err) {
      setError(describeHttpError(err));
      setBusy(false);
    }
  };

  const title = target.kind === "session" ? `Kick and ban ${target.label}` : "Ban an address";
  return (
    <Dialog title={title} onClose={onClose}>
      <form onSubmit={submit} noValidate className="flex flex-col gap-4">
        {error && <Alert>{error}</Alert>}
        {target.kind === "session" ? (
          <p className="text-sm text-fg-muted">
            The session ends and its exact network address is blocked from the whole API, including
            sign-in and guest entry.
          </p>
        ) : (
          <TextField
            label="Address or network"
            name="ban-network"
            autoComplete="off"
            spellCheck={false}
            placeholder="203.0.113.7 or 203.0.113.0/24"
            value={network}
            error={submitted && networkMissing ? "Enter an address or network." : null}
            hint={
              target.hint ??
              "A single address, or a network in CIDR notation (at least /8 for IPv4, /32 for IPv6)."
            }
            onChange={(e) => setNetwork(e.target.value)}
          />
        )}
        <Select label="Duration" value={duration} onChange={setDuration} options={DURATIONS} />
        <TextField
          label="Reason (optional)"
          name="ban-reason"
          maxLength={200}
          autoComplete="off"
          value={reason}
          hint="Only visible to admins."
          onChange={(e) => setReason(e.target.value)}
        />
        {target.kind === "network" && (
          <Toggle label="End sessions from this network now" checked={kick} onChange={setKick} />
        )}
        <div className="flex justify-end gap-2">
          <Button onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" variant="danger" disabled={busy}>
            {busy ? "Banning…" : target.kind === "session" ? "Kick and ban" : "Ban"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
