import { useEffect, useRef, useState } from "react";
import { describeHttpError } from "../http";
import { cn } from "../ui";
import type { PluginInstance } from "./api";
import { CONFIG_FIELDS, buildConfig, configToDraft, type ConfigDraft } from "./model";
import { ConfigRow } from "./StartDialog";
import type { PluginStore } from "./store";

/** Pause after the last edit before the instance is reconfigured (text fields, quick clicks). */
export const APPLY_DELAY_MS = 600;

type Status =
  | { kind: "idle" }
  | { kind: "pending" }
  | { kind: "applying" }
  | { kind: "applied" }
  | { kind: "error"; text: string };

/** Whether the plugin has settings the UI can edit. */
export const hasSettings = (pluginId: string): boolean =>
  (CONFIG_FIELDS[pluginId]?.length ?? 0) > 0;

/**
 * Inline settings of a running instance. Every change is applied live after a short pause:
 * the host restarts the decoder with the new configuration (same instance, counters and history
 * kept); the decoder is down for a moment while it restarts.
 */
export function InstanceSettings({
  instance,
  store,
  className,
}: {
  instance: PluginInstance;
  store: PluginStore;
  className?: string;
}) {
  const fields = CONFIG_FIELDS[instance.pluginId] ?? [];
  const [draft, setDraft] = useState<ConfigDraft>(() => configToDraft(fields, instance.config));
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const dirty = useRef(false);
  const latest = useRef(instance);
  useEffect(() => {
    latest.current = instance;
  });

  // Another browser (or a restore) changed the config: follow it unless the user is typing.
  useEffect(() => {
    if (!dirty.current) setDraft(configToDraft(fields, instance.config));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [instance.config]);
  useEffect(() => () => clearTimeout(timer.current), []);

  const apply = async (next: ConfigDraft) => {
    const built = buildConfig(fields, next);
    if (!built.ok) {
      setStatus({ kind: "error", text: built.error });
      return;
    }
    setStatus({ kind: "applying" });
    try {
      await store.reconfigureInstance(latest.current.id, built.config);
      dirty.current = false;
      setStatus({ kind: "applied" });
    } catch (e) {
      setStatus({ kind: "error", text: describeHttpError(e) });
    }
  };

  const change = (key: string, value: ConfigDraft[string]) => {
    const next = { ...draft, [key]: value };
    setDraft(next);
    dirty.current = true;
    setStatus({ kind: "pending" });
    clearTimeout(timer.current);
    timer.current = setTimeout(() => void apply(next), APPLY_DELAY_MS);
  };

  if (fields.length === 0) {
    return <p className={cn("text-xs text-fg-subtle", className)}>This decoder has no settings.</p>;
  }
  return (
    <fieldset className={cn("m-0 flex flex-col gap-3 border-0 p-0", className)}>
      <legend className="sr-only">Decoder settings</legend>
      {fields.map((f) => (
        <ConfigRow key={f.key} field={f} value={draft[f.key]} onChange={(v) => change(f.key, v)} />
      ))}
      <p
        role="status"
        aria-live="polite"
        className={cn(
          "min-h-4 text-xs",
          status.kind === "error" ? "text-danger" : "text-fg-subtle",
        )}
      >
        {status.kind === "applying" && "Restarting the decoder with the new settings…"}
        {status.kind === "applied" && "Applied. The decoder restarted with the new settings."}
        {status.kind === "error" && status.text}
        {status.kind === "pending" && "Applying in a moment…"}
      </p>
    </fieldset>
  );
}
