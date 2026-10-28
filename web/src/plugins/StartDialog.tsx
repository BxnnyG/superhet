import { useState } from "react";
import { describeHttpError } from "../http";
import { formatMhz } from "../decoders/events";
import {
  Alert,
  Button,
  Dialog,
  Field,
  NumberField,
  Select,
  TextField,
  Toggle,
  toasts,
} from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import type { PluginManifest } from "./api";
import {
  CONFIG_FIELDS,
  buildConfig,
  defaultDraft,
  describeClaim,
  describeInput,
  noticeAccepted,
  parseConfigJson,
  rememberNotice,
  suggestVfo,
  type ConfigDraft,
  type ConfigField,
} from "./model";
import type { PluginStore } from "./store";

interface Props {
  plugin: PluginManifest;
  /** The receiver the decoder listens to; null for exclusive plugins. */
  vfo: Vfo | null;
  device: string;
  store: PluginStore;
  onUpdateVfo: (id: number, patch: VfoPatch) => void;
  onClose: () => void;
}

/**
 * Start a plugin: the licence / legal notice (first time in this browser), its config as a form
 * (keys of the first-party plugins; JSON for others), settings the VFO should have (with an Apply
 * button) and, for exclusive plugins, the explanation that the whole device is taken over.
 */
export function StartDialog({ plugin, vfo, device, store, onUpdateVfo, onClose }: Props) {
  const exclusive = plugin.device.exclusive;
  const fields = CONFIG_FIELDS[plugin.id];
  const [draft, setDraft] = useState<ConfigDraft>(() => defaultDraft(fields ?? []));
  const [json, setJson] = useState("");
  const noticeNeeded = !noticeAccepted(plugin);
  const [noticeRead, setNoticeRead] = useState(!noticeNeeded);
  const [takeOver, setTakeOver] = useState(!exclusive);
  const fixed = plugin.device.centerHz;
  const min = plugin.device.centerHzMin;
  const max = plugin.device.centerHzMax;
  const [centerHz, setCenterHz] = useState<number>(() => fixed ?? min ?? vfo?.freqHz ?? 0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const suggestion = vfo !== null ? suggestVfo(plugin, vfo) : null;
  const needsVfo = !exclusive;
  const canStart = !busy && noticeRead && takeOver && (!needsVfo || vfo !== null);

  const start = async () => {
    const built = fields ? buildConfig(fields, draft) : parseConfigJson(json);
    if (!built.ok) {
      setError(built.error);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await store.startInstance({
        pluginId: plugin.id,
        device,
        ...(needsVfo && vfo !== null ? { vfo: vfo.id, freqHz: vfo.freqHz } : {}),
        ...(exclusive && fixed === null ? { freqHz: centerHz } : {}),
        config: built.config,
      });
      rememberNotice(plugin);
      toasts.push({
        tone: "success",
        title: `${plugin.name} started`,
        key: `plugin-start-${plugin.id}`,
        durationMs: 3000,
      });
      onClose();
    } catch (e) {
      setError(describeHttpError(e));
      setBusy(false);
    }
  };

  const claim = describeClaim(plugin);
  return (
    <Dialog
      title={exclusive ? `Take over ${device} for ${plugin.name}` : `Start ${plugin.name}`}
      size="lg"
      onClose={busy ? () => undefined : onClose}
    >
      <div className="flex flex-col gap-4">
        <p className="text-sm text-fg-muted">
          {plugin.description}{" "}
          <span className="text-fg-subtle">
            ({plugin.license}, version {plugin.version}; input: {describeInput(plugin)})
          </span>
        </p>

        {exclusive && (
          <Alert tone="warn">
            <p className="font-medium">This decoder needs the whole device.</p>
            <p className="mt-1">
              While it runs, {device} is tuned to {claim ?? "its own frequency"} and nothing else
              can retune it: your receivers outside that window are paused, other users cannot tune,
              and the tuning lock is held by the decoder. Stopping it restores the previous tuning.
            </p>
            <div className="mt-3">
              <Toggle
                label="I understand: take over the device"
                checked={takeOver}
                onChange={setTakeOver}
              />
            </div>
          </Alert>
        )}

        {plugin.acknowledge !== null && (
          <section
            aria-label="Notice"
            className="rounded-lg border border-border-strong bg-surface-2 px-3 py-2.5"
          >
            <h3 className="text-xs font-semibold tracking-wider text-fg-muted uppercase">
              Licence and legal notice
            </h3>
            <p className="mt-1.5 text-sm text-fg">{plugin.acknowledge.text}</p>
            {plugin.acknowledge.url !== null && (
              <p className="mt-1.5 text-xs">
                <a
                  href={plugin.acknowledge.url}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-accent underline-offset-2 hover:underline"
                >
                  {plugin.acknowledge.url}
                </a>
              </p>
            )}
            {noticeNeeded ? (
              <div className="mt-3">
                <Toggle
                  label="I have read this notice and use the decoder only where I am allowed to"
                  checked={noticeRead}
                  onChange={setNoticeRead}
                />
              </div>
            ) : (
              <p className="mt-2 text-xs text-fg-subtle">You confirmed this notice before.</p>
            )}
          </section>
        )}

        {needsVfo && vfo === null && (
          <Alert tone="warn">Select a receiver first: the decoder listens to one VFO.</Alert>
        )}

        {suggestion !== null && vfo !== null && (
          <Alert tone="info">
            <p className="font-medium">Suggested settings for VFO {vfo.id}</p>
            <ul className="mt-1 list-disc pl-4">
              {suggestion.lines.map((l) => (
                <li key={l}>{l}</li>
              ))}
            </ul>
            <Button
              size="sm"
              className="mt-2"
              onClick={() => onUpdateVfo(vfo.id, suggestion.patch)}
            >
              Apply to VFO {vfo.id}
            </Button>
          </Alert>
        )}

        {exclusive && fixed === null && min !== null && max !== null && (
          <NumberField
            label={`Centre frequency (${formatMhz(min)} to ${formatMhz(max)} MHz)`}
            value={centerHz}
            min={min}
            max={max}
            decimals={0}
            unit="MHz"
            format={(v) => formatMhz(v)}
            parse={(t) => {
              const n = Number(t.trim().replace(",", "."));
              return Number.isFinite(n) ? Math.round(n * 1e6) : null;
            }}
            onCommit={setCenterHz}
          />
        )}

        {fields !== undefined ? (
          fields.length > 0 && (
            <fieldset className="flex flex-col gap-3">
              <legend className="mb-1 text-xs font-semibold tracking-wider text-fg-muted uppercase">
                Settings
              </legend>
              {fields.map((f) => (
                <ConfigRow
                  key={f.key}
                  field={f}
                  value={draft[f.key]}
                  onChange={(v) => setDraft((d) => ({ ...d, [f.key]: v }))}
                />
              ))}
            </fieldset>
          )
        ) : (
          <div className="flex flex-col gap-1">
            <label htmlFor="plugin-config-json" className="text-xs font-medium text-fg-muted">
              Config (JSON, optional)
            </label>
            <textarea
              id="plugin-config-json"
              rows={4}
              value={json}
              spellCheck={false}
              placeholder="{}"
              onChange={(e) => setJson(e.target.value)}
              className="focus-ring w-full resize-y rounded-md border border-border-strong bg-surface-1 px-3 py-2 font-mono text-sm text-fg placeholder:text-fg-subtle"
            />
            <p className="text-xs text-fg-subtle">
              Passed to the plugin as it is. Unknown keys can make the plugin refuse to start.
            </p>
          </div>
        )}

        {error !== null && <Alert>{error}</Alert>}

        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void start()} disabled={!canStart}>
            {busy ? "Starting…" : exclusive ? "Take over and start" : "Start"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

export function ConfigRow({
  field,
  value,
  onChange,
}: {
  field: ConfigField;
  value: ConfigDraft[string] | undefined;
  onChange: (v: ConfigDraft[string]) => void;
}) {
  if (field.type === "bool") {
    return (
      <div className="flex flex-col gap-0.5">
        <Toggle label={field.label} checked={value === true} onChange={onChange} />
        {field.hint && <p className="text-xs text-fg-subtle">{field.hint}</p>}
      </div>
    );
  }
  if (field.type === "multi") {
    const chosen = Array.isArray(value) ? (value as readonly number[]) : [];
    return (
      <Field label={field.label}>
        <div className="flex flex-wrap gap-3" role="group" aria-label={field.label}>
          {field.options.map((o) => (
            <label key={o} className="inline-flex items-center gap-1.5 text-sm text-fg">
              <input
                type="checkbox"
                checked={chosen.includes(o)}
                onChange={(e) =>
                  onChange(e.target.checked ? [...chosen, o] : chosen.filter((x) => x !== o))
                }
                className="size-4 accent-accent"
              />
              {o}
            </label>
          ))}
        </div>
        {field.hint && <p className="text-xs text-fg-subtle">{field.hint}</p>}
      </Field>
    );
  }
  if (field.type === "select") {
    return (
      <div className="flex flex-col gap-1">
        <Select
          label={field.label}
          options={field.options}
          value={typeof value === "string" ? value : ""}
          onChange={onChange}
        />
        {field.hint && <p className="text-xs text-fg-subtle">{field.hint}</p>}
      </div>
    );
  }
  return (
    <TextField
      label={field.label}
      inputMode="numeric"
      value={typeof value === "string" ? value : ""}
      placeholder={field.placeholder}
      hint={field.hint}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
