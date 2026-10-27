import { useCallback, useState, type SyntheticEvent } from "react";
import { describeHttpError } from "../http";
import {
  describeBinding,
  describeClaim,
  describeInput,
  instanceProblem,
  isBundled,
} from "../plugins/model";
import { StateBadge } from "../plugins/parts";
import { pluginName } from "../plugins/store";
import type { PluginApi, PluginInstance, PluginManifest } from "../plugins/api";
import {
  Alert,
  Badge,
  Button,
  ConfirmDialog,
  Dialog,
  EmptyState,
  PlusIcon,
  RefreshIcon,
  StopIcon,
  TextField,
  TrashIcon,
  toasts,
} from "../ui";
import { Card, LoadState, SectionHeader, useNow } from "./common";
import { formatRelative } from "../format";
import { useLoad } from "./useLoad";

const POLL_MS = 3000;

/** Admin console section: installed plugins and the instances that run them. */
export function PluginsSection({ api }: { api: PluginApi }) {
  const loadPlugins = useCallback((s: AbortSignal) => api.listPlugins(s), [api]);
  const loadInstances = useCallback((s: AbortSignal) => api.listInstances(s), [api]);
  const plugins = useLoad(loadPlugins);
  const instances = useLoad(loadInstances, POLL_MS);
  const [installing, setInstalling] = useState(false);
  const [removing, setRemoving] = useState<PluginManifest | null>(null);
  const [busy, setBusy] = useState(false);

  const reloadAll = () => {
    plugins.reload();
    instances.reload();
  };

  const remove = async (p: PluginManifest) => {
    setBusy(true);
    try {
      await api.remove(p.id);
      toasts.push({ tone: "success", title: `${p.name} removed` });
      setRemoving(null);
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: `Could not remove ${p.name}`,
        body: describeHttpError(e),
      });
      setRemoving(null);
    } finally {
      setBusy(false);
      reloadAll();
    }
  };

  const list = plugins.data ?? [];
  const running = instances.data ?? [];
  return (
    <div>
      <SectionHeader
        title="Plugins"
        description="Decoders run as separate processes. The first-party decoders ship with the image; other plugins are installed from a directory on the plugin host."
        actions={
          <>
            <Button icon={<RefreshIcon size={14} />} onClick={reloadAll}>
              Refresh
            </Button>
            <Button
              variant="primary"
              icon={<PlusIcon size={14} />}
              onClick={() => setInstalling(true)}
            >
              Install from path
            </Button>
          </>
        }
      />

      <Card
        title="Installed plugins"
        description={plugins.data ? `${list.length} installed` : undefined}
      >
        <LoadState loading={plugins.loading} error={plugins.error} onRetry={plugins.reload} />
        {plugins.data && list.length === 0 && (
          <div className="p-4">
            <EmptyState title="No plugins installed">
              Install one from a directory that contains a plugin.toml.
            </EmptyState>
          </div>
        )}
        {plugins.data && list.length > 0 && (
          <ul className="divide-y divide-border" aria-label="Installed plugins">
            {list.map((p) => (
              <PluginRow
                key={p.id}
                plugin={p}
                instances={running.filter((i) => i.pluginId === p.id).length}
                onRemove={() => setRemoving(p)}
              />
            ))}
          </ul>
        )}
      </Card>

      <Card
        className="mt-6"
        title="Instances"
        description="Running decoders. Start them from the Decoders tab of the receiver."
      >
        <LoadState loading={instances.loading} error={instances.error} onRetry={instances.reload} />
        {instances.data && running.length === 0 && (
          <div className="p-4">
            <EmptyState title="Nothing is running">
              Pick a VFO and start a decoder in the Decoders tab.
            </EmptyState>
          </div>
        )}
        {instances.data && running.length > 0 && (
          <InstanceTable
            api={api}
            instances={running}
            plugins={list}
            onChanged={instances.reload}
          />
        )}
      </Card>

      {installing && (
        <InstallDialog api={api} onClose={() => setInstalling(false)} onInstalled={reloadAll} />
      )}
      {removing && (
        <ConfirmDialog
          title={`Remove ${removing.name}?`}
          confirmLabel="Remove"
          confirmVariant="danger"
          busy={busy}
          onCancel={() => setRemoving(null)}
          onConfirm={() => void remove(removing)}
        >
          All installed versions are deleted. Its stored events stay in the decoder history. A
          plugin with a running instance cannot be removed: stop the instance first.
        </ConfirmDialog>
      )}
    </div>
  );
}

function PluginRow({
  plugin: p,
  instances,
  onRemove,
}: {
  plugin: PluginManifest;
  instances: number;
  onRemove: () => void;
}) {
  const claim = describeClaim(p);
  return (
    <li className="px-4 py-3" data-testid={`plugin-${p.id}`}>
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-sm font-medium">{p.name}</span>
            <Badge title="Version">v{p.version}</Badge>
            <Badge tone="accent" title="Plugin type">
              {p.type}
            </Badge>
            <Badge title="Licence of the plugin (SPDX)">{p.license}</Badge>
            {isBundled(p.id) && (
              <Badge tone="success" title="Ships with the image and is installed on first start">
                Bundled
              </Badge>
            )}
            {p.device.exclusive && (
              <Badge tone="warn" title="Takes over the whole device while it runs">
                Whole device
              </Badge>
            )}
            {instances > 0 && <Badge tone="success">{instances} running</Badge>}
          </div>
          <p className="mt-0.5 font-mono text-xs text-fg-subtle">{p.id}</p>
          <p className="mt-1 text-sm text-fg-muted">{p.description}</p>
          <p className="mt-1 text-xs text-fg-subtle">
            Input: {describeInput(p)}
            {claim !== null && ` · claims ${claim}`}
            {p.transport !== "simple" && ` · ${p.transport} transport`}
            {p.runtimeKind !== "process" && ` · ${p.runtimeKind}`}
          </p>
          {p.homepage !== null && (
            <p className="mt-1 text-xs">
              <a
                href={p.homepage}
                target="_blank"
                rel="noopener noreferrer"
                className="text-accent underline-offset-2 hover:underline"
              >
                {p.homepage}
              </a>
            </p>
          )}
        </div>
        <Button
          size="sm"
          variant="danger"
          icon={<TrashIcon size={12} />}
          aria-label={`Remove ${p.name}`}
          onClick={onRemove}
        >
          Remove
        </Button>
      </div>
      {p.acknowledge !== null && (
        <details className="mt-2 text-xs text-fg-muted">
          <summary className="cursor-pointer select-none">Install notice</summary>
          <p className="mt-1.5 max-w-3xl text-sm text-fg">{p.acknowledge.text}</p>
          {p.acknowledge.url !== null && (
            <a
              href={p.acknowledge.url}
              target="_blank"
              rel="noopener noreferrer"
              className="mt-1 inline-block text-accent underline-offset-2 hover:underline"
            >
              {p.acknowledge.url}
            </a>
          )}
          <p className="mt-1.5 text-fg-subtle">
            Operators confirm this text once per browser before the first start.
          </p>
        </details>
      )}
      {p.build.length > 0 && (
        <p className="mt-2 text-xs text-fg-subtle">
          Build commands (shown, never run by the host): {p.build.join(", ")}
        </p>
      )}
    </li>
  );
}

function InstanceTable({
  api,
  instances,
  plugins,
  onChanged,
}: {
  api: PluginApi;
  instances: readonly PluginInstance[];
  plugins: readonly PluginManifest[];
  onChanged: () => void;
}) {
  const [busyId, setBusyId] = useState<string | null>(null);
  const now = useNow(15_000);
  const run = async (i: PluginInstance, what: "stop" | "restart") => {
    setBusyId(i.id);
    try {
      if (what === "stop") await api.stop(i.id);
      else await api.restart(i.id);
    } catch (e) {
      toasts.push({
        tone: "danger",
        title: `Could not ${what} the instance`,
        body: describeHttpError(e),
      });
    } finally {
      setBusyId(null);
      onChanged();
    }
  };
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[40rem] text-left text-sm" aria-label="Plugin instances">
        <thead className="text-xs text-fg-subtle">
          <tr className="border-b border-border">
            <th scope="col" className="px-4 py-2 font-medium">
              Plugin
            </th>
            <th scope="col" className="px-2 py-2 font-medium">
              State
            </th>
            <th scope="col" className="px-2 py-2 font-medium">
              Bound to
            </th>
            <th scope="col" className="px-2 py-2 text-right font-medium">
              Restarts
            </th>
            <th scope="col" className="px-2 py-2 text-right font-medium">
              Input dropped
            </th>
            <th scope="col" className="px-2 py-2 font-medium">
              Problem
            </th>
            <th scope="col" className="px-4 py-2 text-right font-medium">
              <span className="sr-only">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody className="divide-y divide-border">
          {instances.map((i) => {
            const problem = instanceProblem(i);
            const name = pluginName(plugins, i.pluginId);
            return (
              <tr key={i.id} data-testid={`instance-${i.id}`} className="align-top">
                <td className="px-4 py-2.5">
                  <div className="font-medium">{name}</div>
                  <div className="font-mono text-[11px] text-fg-subtle">{i.id}</div>
                </td>
                <td className="px-2 py-2.5">
                  <StateBadge state={i.state} />
                  {i.updatedMs > 0 && (
                    <div className="mt-0.5 text-[11px] text-fg-subtle">
                      {formatRelative(i.updatedMs, now)}
                    </div>
                  )}
                </td>
                <td className="px-2 py-2.5 text-xs text-fg-muted">{describeBinding(i)}</td>
                <td className="px-2 py-2.5 text-right num">
                  {i.restarts}
                  {i.crashesInWindow > 0 && (
                    <div className="text-[11px] text-fg-subtle">{i.crashesInWindow} recent</div>
                  )}
                </td>
                <td className="px-2 py-2.5 text-right num">{i.inputDropped}</td>
                <td className="max-w-64 px-2 py-2.5 text-xs">
                  {problem === null ? (
                    <span className="text-fg-subtle">–</span>
                  ) : (
                    <span className="line-clamp-3 break-words text-danger" title={problem}>
                      {problem}
                    </span>
                  )}
                </td>
                <td className="px-4 py-2.5">
                  <div className="flex justify-end gap-1.5">
                    {i.state === "error" && (
                      <Button
                        size="sm"
                        icon={<RefreshIcon size={12} />}
                        disabled={busyId === i.id}
                        onClick={() => void run(i, "restart")}
                      >
                        Restart
                      </Button>
                    )}
                    <Button
                      size="sm"
                      icon={<StopIcon size={12} />}
                      disabled={busyId === i.id}
                      aria-label={`Stop ${name} (${i.id})`}
                      onClick={() => void run(i, "stop")}
                    >
                      Stop
                    </Button>
                  </div>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function InstallDialog({
  api,
  onClose,
  onInstalled,
}: {
  api: PluginApi;
  onClose: () => void;
  onInstalled: () => void;
}) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<PluginManifest | null>(null);

  const submit = async (e: SyntheticEvent) => {
    e.preventDefault();
    if (path.trim() === "") return;
    setBusy(true);
    setError(null);
    try {
      const installed = await api.install(path.trim());
      setDone(installed);
      onInstalled();
    } catch (err) {
      setError(describeHttpError(err));
    } finally {
      setBusy(false);
    }
  };

  if (done !== null) {
    return (
      <ConfirmDialog title={`${done.name} installed`} confirmLabel="Done" onCancel={onClose}>
        <p>
          Version {done.version} ({done.license}) is available in the Decoders tab.
        </p>
        {done.acknowledge !== null && (
          <p className="mt-2 text-fg">
            <span className="font-medium">Notice for users: </span>
            {done.acknowledge.text}
          </p>
        )}
      </ConfirmDialog>
    );
  }
  return (
    <Dialog title="Install a plugin" onClose={busy ? () => undefined : onClose}>
      <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3">
        <TextField
          label="Directory with plugin.toml"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          placeholder="/plugins-src/my-plugin"
          spellCheck={false}
          autoComplete="off"
          hint="A path on the machine (or in the container) that runs the plugin host. The directory is validated and copied; symbolic links are refused and build commands are not run."
        />
        {error !== null && <Alert>{error}</Alert>}
        <div className="flex justify-end gap-2">
          <Button variant="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" disabled={busy || path.trim() === ""}>
            {busy ? "Installing…" : "Install"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
