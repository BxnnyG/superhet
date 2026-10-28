// Exclusive plugins (DAB, ADS-B) take the whole device: they hold a `claim` on it, move the
// centre, and every VFO outside their window pauses. These helpers name the owner of a claim for
// the tuning strip and explain a refused start ("device is already claimed by instance i-...")
// in words, with the way out.

import type { LockHolder } from "../spectrum/api";
import type { PluginInstance, PluginManifest, StartRequest } from "./api";
import type { PluginStore } from "./store";

/** Short names for the plugins whose manifest names are long; others use their manifest name. */
const SHORT_NAME: Record<string, string> = {
  "org.superhet.decoder.dab": "DAB",
  "org.superhet.decoder.adsb": "ADS-B",
};

export function shortPluginName(plugins: readonly PluginManifest[], pluginId: string): string {
  return (
    SHORT_NAME[pluginId] ??
    plugins.find((p) => p.id === pluginId)?.name ??
    pluginId.split(".").slice(-1)[0] ??
    pluginId
  );
}

/** The plugin instance that holds the claim on the device. */
export interface ClaimOwner {
  /** Instance id named by the claim lock. */
  instanceId: string;
  /** Plugin id when it is known (operators know every instance; others only the followed one). */
  pluginId: string | null;
  /** "DAB", "ADS-B", or a neutral "A plugin" when the viewer cannot tell. */
  name: string;
}

/** An instance id together with the plugin it belongs to, as the DAB / ADS-B stores know it. */
export interface KnownInstance {
  instanceId: string | null;
  pluginId: string | null;
}

/**
 * Who holds the device claim, or null when nobody does. Operators resolve the owner from the
 * instance list; everybody else from the status of the instance the DAB and ADS-B views follow.
 */
export function claimOwner(
  locks: readonly LockHolder[],
  instances: readonly PluginInstance[],
  plugins: readonly PluginManifest[],
  known: readonly KnownInstance[] = [],
): ClaimOwner | null {
  const claim = locks.find((l) => l.kind === "claim");
  if (claim === undefined) return null;
  const pluginId =
    instances.find((i) => i.id === claim.id)?.pluginId ??
    known.find((k) => k.instanceId === claim.id)?.pluginId ??
    null;
  return {
    instanceId: claim.id,
    pluginId,
    name: pluginId === null ? "A plugin" : shortPluginName(plugins, pluginId),
  };
}

/** "DAB has the device" / "A plugin has the device". */
export function claimBadge(owner: ClaimOwner): string {
  return `${owner.name} has the device`;
}

/** Tooltip for a VFO that waits while a plugin owns the device. */
export function claimHint(owner: ClaimOwner, centreHz: number | null, vfoHz: number): string {
  const at = centreHz === null ? "" : ` at ${(centreHz / 1e6).toFixed(3)} MHz`;
  return (
    `${owner.name} is using the whole device${at}. ` +
    `This receiver keeps ${(vfoHz / 1e6).toFixed(4)} MHz and resumes when ${owner.name} stops.`
  );
}

/** A running exclusive instance that stands in the way of starting `wantedPluginId`. */
export function blockingInstance(
  instances: readonly PluginInstance[],
  device: string,
  wantedPluginId: string,
): PluginInstance | null {
  return (
    instances.find(
      (i) =>
        i.exclusive &&
        i.device === device &&
        i.pluginId !== wantedPluginId &&
        i.state !== "stopped" &&
        i.state !== "error",
    ) ?? null
  );
}

/** Wording of the start conflict for the blocking instance. */
export function conflictText(
  blocker: PluginInstance,
  plugins: readonly PluginManifest[],
  wanted: string,
): string {
  const other = shortPluginName(plugins, blocker.pluginId);
  return `${other} is using the device. Stop ${other} to start ${wanted}.`;
}

/**
 * Start an instance; when another exclusive plugin holds the device, stop it first (the user
 * confirmed "Stop X and start Y"). Rejects with the server's message.
 */
export async function startReplacing(
  plugins: Pick<PluginStore, "startInstance" | "stopInstance">,
  blocker: PluginInstance | null,
  req: StartRequest,
): Promise<PluginInstance> {
  if (blocker !== null) await plugins.stopInstance(blocker.id);
  return plugins.startInstance(req);
}
