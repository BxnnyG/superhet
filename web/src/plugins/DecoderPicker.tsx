import { useState, useSyncExternalStore } from "react";
import { describeHttpError } from "../http";
import { navigate } from "../router";
import {
  Alert,
  Badge,
  Button,
  ChevronDownIcon,
  EmptyState,
  PlainButton,
  PlayIcon,
  RefreshIcon,
  StopIcon,
  cn,
  toasts,
} from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import type { PluginInstance, PluginManifest } from "./api";
import { describeBinding, describeInput, instanceProblem, suggestVfo } from "./model";
import { StateBadge } from "./parts";
import { StartDialog } from "./StartDialog";
import { instancesOnDevice, instancesOnVfo, pluginName, type PluginStore } from "./store";

interface Props {
  store: PluginStore;
  device: string;
  /** The selected receiver; null when there is none. */
  vfo: Vfo | null;
  isAdmin: boolean;
  onUpdateVfo: (id: number, patch: VfoPatch) => void;
}

const OPEN_KEY = "superhet.decoder-picker-open";

function readOpen(): boolean {
  try {
    return localStorage.getItem(OPEN_KEY) !== "0";
  } catch {
    return true;
  }
}

/** Plugins an operator can start from here: decoders run by the host as a plain process. */
export function startable(p: PluginManifest): boolean {
  return p.type === "decoder" && p.runtimeKind === "process" && p.transport === "simple";
}

/**
 * "Decoders on this VFO": the running decoder plugins of the selected receiver (state, restarts,
 * errors, stop / restart) and the installed ones that can be started on it. Exclusive plugins,
 * which take over the whole device, are listed apart.
 */
export function DecoderPicker({ store, device, vfo, isAdmin, onUpdateVfo }: Props) {
  const snap = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const [open, setOpenState] = useState(() => readOpen());
  const setOpen = (next: boolean) => {
    setOpenState(next);
    try {
      localStorage.setItem(OPEN_KEY, next ? "1" : "0");
    } catch {
      // Private mode: the section opens again next time.
    }
  };
  const [starting, setStarting] = useState<PluginManifest | null>(null);

  const plugins = snap.plugins.filter(startable);
  const shared = plugins.filter((p) => !p.device.exclusive);
  const exclusive = plugins.filter((p) => p.device.exclusive);
  const here = vfo === null ? [] : instancesOnVfo(snap.instances, device, vfo.id);
  const deviceWide = instancesOnDevice(snap.instances, device);
  const running = here.length + deviceWide.length;

  return (
    <section aria-label="Decoder plugins" className="border-b border-border">
      <h3 className="m-0">
        <PlainButton
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          className="focus-ring flex h-10 w-full items-center gap-1.5 px-3 text-left text-xs font-semibold tracking-wider text-fg-muted uppercase transition-colors duration-150 hover:text-fg"
        >
          <ChevronDownIcon
            size={14}
            className={cn("transition-transform duration-150", !open && "-rotate-90")}
          />
          <span className="min-w-0 flex-1 truncate">
            {vfo === null ? "Decoder plugins" : `Decoders on VFO ${vfo.id}`}
          </span>
          {running > 0 && <Badge tone="success">{running} running</Badge>}
        </PlainButton>
      </h3>
      {open && (
        <div className="flex flex-col gap-3 px-3 pb-3">
          {snap.error !== null && (
            <Alert tone="warn">
              <p>Plugins are unavailable: {snap.error}</p>
              <Button
                size="sm"
                className="mt-2"
                icon={<RefreshIcon size={12} />}
                onClick={() => void store.refresh()}
              >
                Retry
              </Button>
            </Alert>
          )}
          {!snap.loaded && snap.error === null && (
            <p className="text-xs text-fg-subtle">Loading…</p>
          )}

          {here.length > 0 && (
            <ul aria-label="Running on this VFO" className="flex flex-col gap-2">
              {here.map((i) => (
                <InstanceRow key={i.id} instance={i} plugins={snap.plugins} store={store} />
              ))}
            </ul>
          )}

          {snap.loaded && snap.error === null && plugins.length === 0 && (
            <EmptyState title="No decoder plugins installed">
              {isAdmin ? (
                <>
                  Install one in the{" "}
                  <a
                    href="/admin/plugins"
                    onClick={(e) => {
                      e.preventDefault();
                      navigate("/admin/plugins");
                    }}
                    className="text-accent underline-offset-2 hover:underline"
                  >
                    admin console
                  </a>
                  .
                </>
              ) : (
                "An admin can install them in the admin console."
              )}
            </EmptyState>
          )}

          {shared.length > 0 && (
            <ul aria-label="Available decoders" className="flex flex-col divide-y divide-border">
              {shared.map((p) => (
                <AvailableRow
                  key={p.id}
                  plugin={p}
                  vfo={vfo}
                  runningHere={here.some((i) => i.pluginId === p.id)}
                  onStart={() => setStarting(p)}
                />
              ))}
            </ul>
          )}

          {(exclusive.length > 0 || deviceWide.length > 0) && (
            <div className="flex flex-col gap-2">
              <h4 className="text-[11px] font-semibold tracking-wider text-fg-subtle uppercase">
                Takes over the device
              </h4>
              {deviceWide.length > 0 && (
                <ul aria-label="Running on the device" className="flex flex-col gap-2">
                  {deviceWide.map((i) => (
                    <InstanceRow key={i.id} instance={i} plugins={snap.plugins} store={store} />
                  ))}
                </ul>
              )}
              <ul aria-label="Exclusive decoders" className="flex flex-col divide-y divide-border">
                {exclusive.map((p) => (
                  <AvailableRow
                    key={p.id}
                    plugin={p}
                    vfo={null}
                    runningHere={deviceWide.some((i) => i.pluginId === p.id)}
                    onStart={() => setStarting(p)}
                  />
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
      {starting !== null && (
        <StartDialog
          plugin={starting}
          vfo={starting.device.exclusive ? null : vfo}
          device={device}
          store={store}
          onUpdateVfo={onUpdateVfo}
          onClose={() => setStarting(null)}
        />
      )}
    </section>
  );
}

function AvailableRow({
  plugin,
  vfo,
  runningHere,
  onStart,
}: {
  plugin: PluginManifest;
  vfo: Vfo | null;
  runningHere: boolean;
  onStart: () => void;
}) {
  const exclusive = plugin.device.exclusive;
  const fit = vfo !== null && !exclusive ? suggestVfo(plugin, vfo) : null;
  const blocked = !exclusive && vfo === null;
  return (
    <li className="flex items-start gap-2 py-2" data-testid={`plugin-${plugin.id}`}>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-sm font-medium text-fg">{plugin.name}</span>
          {exclusive && <Badge tone="warn">Whole device</Badge>}
          {fit !== null && (
            <Badge tone="accent" title={fit.lines.join("\n")}>
              Needs settings
            </Badge>
          )}
        </div>
        <p className="mt-0.5 line-clamp-1 text-xs text-fg-muted" title={plugin.description}>
          {plugin.description}
        </p>
        <p className="mt-0.5 text-[11px] text-fg-subtle">{describeInput(plugin)}</p>
      </div>
      <Button
        size="sm"
        icon={<PlayIcon size={12} />}
        disabled={blocked}
        title={blocked ? "Select a receiver first" : undefined}
        aria-label={`Start ${plugin.name}`}
        onClick={onStart}
      >
        {runningHere ? "Add another" : "Start…"}
      </Button>
    </li>
  );
}

function InstanceRow({
  instance: i,
  plugins,
  store,
}: {
  instance: PluginInstance;
  plugins: readonly PluginManifest[];
  store: PluginStore;
}) {
  const [busy, setBusy] = useState(false);
  const name = pluginName(plugins, i.pluginId);
  const problem = instanceProblem(i);
  const run = async (what: "stop" | "restart") => {
    setBusy(true);
    try {
      if (what === "stop") await store.stopInstance(i.id);
      else await store.restartInstance(i.id);
    } catch (e) {
      toasts.push({
        tone: "warn",
        title: `Could not ${what} ${name}`,
        body: describeHttpError(e),
        key: `plugin-${what}-${i.id}`,
      });
    } finally {
      setBusy(false);
    }
  };
  return (
    <li
      className="rounded-lg border border-border bg-surface-1 px-3 py-2"
      data-testid={`instance-${i.id}`}
    >
      <div className="flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-fg">{name}</span>
        <StateBadge state={i.state} />
        {i.state === "error" && (
          <Button
            size="sm"
            icon={<RefreshIcon size={12} />}
            disabled={busy}
            onClick={() => void run("restart")}
          >
            Restart
          </Button>
        )}
        <Button
          size="sm"
          variant="secondary"
          icon={<StopIcon size={12} />}
          disabled={busy}
          aria-label={`Stop ${name}`}
          onClick={() => void run("stop")}
        >
          Stop
        </Button>
      </div>
      <p className="mt-1 text-[11px] text-fg-subtle">
        {describeBinding(i)}
        {i.restarts > 0 && ` · ${i.restarts} restart${i.restarts === 1 ? "" : "s"}`}
        {i.eventsOk > 0 && ` · ${i.eventsOk} event${i.eventsOk === 1 ? "" : "s"}`}
        {i.inputDropped > 0 && ` · ${i.inputDropped} dropped input chunks`}
      </p>
      {problem !== null && (
        <p className="mt-1 line-clamp-3 text-xs break-words text-danger" title={problem}>
          {problem}
        </p>
      )}
    </li>
  );
}
