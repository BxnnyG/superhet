import { useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import { describeHttpError } from "../http";
import type { PluginManifest } from "../plugins/api";
import { startable } from "../plugins/DecoderPicker";
import type { PluginStore } from "../plugins/store";
import { Alert, Button, Chip, Dialog, Select, Toggle, toasts } from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import { enterDigital, leaveDigital, type EnterContext } from "./actions";
import {
  DIGITAL_MODES,
  digitalOnVfo,
  digitalMode,
  suggestDigital,
  type DigitalMode,
  type DigitalModeId,
} from "./modes";

interface Props {
  vfo: Vfo;
  device: string;
  plugins: PluginStore;
  onUpdate: (id: number, patch: VfoPatch) => void;
}

const ANALOG = "";

/**
 * "Digital" group of the mode bar: POCSAG, APRS and ISM sensors. Picking one sets the VFO up
 * (NFM, the bandwidth the decoder wants, squelch off), starts the plugin on it and the output
 * panel below the spectrum shows what it decodes. "Analog" stops it again. A small chip offers
 * the mode that usually lives on the VFO's frequency.
 */
export function DigitalModeControl({ vfo, device, plugins, onUpdate }: Props) {
  const perms = usePermissions();
  const snap = useSyncExternalStore(plugins.subscribe, plugins.getSnapshot);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<DigitalMode | null>(null);
  if (!perms.canUsePlugins) return null;

  const installed = (m: DigitalMode): PluginManifest | undefined =>
    snap.plugins.find((p) => p.id === m.pluginId && startable(p));
  const available = DIGITAL_MODES.filter((m) => installed(m) !== undefined);
  if (available.length === 0) return null;

  const active = digitalOnVfo(snap.instances, device, vfo.id)[0]?.mode ?? null;
  const ctx: EnterContext = { store: plugins, device, vfo, updateVfo: onUpdate };

  const run = async (mode: DigitalMode, acknowledged = false) => {
    setBusy(true);
    try {
      if ((await enterDigital(ctx, mode, acknowledged)) === "notice") setNotice(mode);
      else setNotice(null);
    } catch (e) {
      setNotice(null);
      toasts.push({
        tone: "warn",
        title: `Could not start ${mode.label}`,
        body: describeHttpError(e),
        key: `digital-start-${mode.id}`,
      });
    } finally {
      setBusy(false);
    }
  };

  const choose = (value: string) => {
    if (value === ANALOG) {
      setBusy(true);
      void leaveDigital(plugins, device, vfo.id)
        .catch((e: unknown) =>
          toasts.push({
            tone: "warn",
            title: "Could not stop the decoder",
            body: describeHttpError(e),
            key: "digital-stop",
          }),
        )
        .finally(() => setBusy(false));
      return;
    }
    void run(digitalMode(value as DigitalModeId));
  };

  const hint = active === null ? suggestDigital(vfo.freqHz) : null;
  const hinted = hint !== null && installed(hint.mode) !== undefined ? hint : null;

  return (
    <>
      <Select
        aria-label="Digital mode"
        size="sm"
        className="w-28 shrink-0"
        disabled={busy}
        title={
          active
            ? `${active.title}. Pick Analog to stop the decoder.`
            : "Decode pagers, APRS or ISM sensors on this receiver"
        }
        value={active?.id ?? ANALOG}
        onChange={choose}
        options={[
          { value: ANALOG, label: active === null ? "Digital…" : "Analog" },
          ...available.map((m) => ({ value: m.id, label: m.label })),
        ]}
      />
      {hinted !== null && (
        <Chip
          selected={false}
          className="max-w-36 truncate"
          disabled={busy}
          aria-label={`Try ${hinted.mode.label} here`}
          title={`${hinted.note}: click to start ${hinted.mode.label} on this receiver`}
          onClick={() => void run(hinted.mode)}
        >
          Try {hinted.mode.label}?
        </Chip>
      )}
      {notice !== null && (
        <NoticeDialog
          mode={notice}
          manifest={installed(notice)}
          busy={busy}
          onCancel={() => setNotice(null)}
          onConfirm={() => void run(notice, true)}
        />
      )}
    </>
  );
}

function NoticeDialog({
  mode,
  manifest,
  busy,
  onCancel,
  onConfirm,
}: {
  mode: DigitalMode;
  manifest: PluginManifest | undefined;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const [read, setRead] = useState(false);
  const ack = manifest?.acknowledge ?? null;
  return (
    <Dialog title={`Start ${mode.label}`} onClose={busy ? () => undefined : onCancel}>
      <div className="flex flex-col gap-4">
        <p className="text-sm text-fg-muted">
          {mode.title}. The receiver is switched to NFM at{" "}
          {mode.vfo.bandwidthHz >= 1000
            ? `${mode.vfo.bandwidthHz / 1000} kHz`
            : mode.vfo.bandwidthHz}{" "}
          with the squelch off.
        </p>
        {ack !== null && (
          <Alert tone="warn">
            <p>{ack.text}</p>
            {ack.url !== null && (
              <p className="mt-1.5 text-xs">
                <a
                  href={ack.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent underline-offset-2 hover:underline"
                >
                  {ack.url}
                </a>
              </p>
            )}
          </Alert>
        )}
        <Toggle
          label="I have read this notice and use the decoder only where I am allowed to"
          checked={read}
          onChange={setRead}
        />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onCancel} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={onConfirm} disabled={!read || busy}>
            {busy ? "Starting…" : `Start ${mode.label}`}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
