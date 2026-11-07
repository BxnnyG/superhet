import { useState, useSyncExternalStore } from "react";
import { usePermissions } from "../auth/context";
import type { Receiver } from "../shell/useReceiver";
import { EmptyState, Select } from "../ui";
import { PluginPanel } from "./PluginPanel";

/**
 * Inspector tab with the panels plugins ship (ADR-0022): one sandboxed frame, a picker when more
 * than one plugin brings a panel. The data comes from the receiver's stores; the frame only ever
 * sees a filtered copy through the bridge.
 */
export function PanelsTab({ r }: { r: Receiver }) {
  const perms = usePermissions();
  const { panels } = useSyncExternalStore(r.panels.subscribe, r.panels.getSnapshot);
  const plugins = useSyncExternalStore(r.plugins.subscribe, r.plugins.getSnapshot);
  const events = useSyncExternalStore(r.decoderEvents.subscribe, r.decoderEvents.getSnapshot);
  const [picked, setPicked] = useState<string | null>(null);

  const panel = panels.find((p) => p.pluginId === picked) ?? panels[0];
  if (!panel) {
    return (
      <div className="p-4">
        <EmptyState title="No plugin panels">Plugins that ship a panel show it here.</EmptyState>
      </div>
    );
  }
  return (
    <div className="flex h-full min-h-0 flex-col">
      {panels.length > 1 && (
        <div className="shrink-0 border-b border-border px-4 py-2">
          <Select
            aria-label="Plugin panel"
            size="sm"
            value={panel.pluginId}
            onChange={setPicked}
            options={panels.map((p) => ({ value: p.pluginId, label: p.title }))}
          />
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col">
        <PluginPanel
          key={`${panel.pluginId}@${panel.version}`}
          panel={panel}
          instances={plugins.instances}
          events={events.events}
          isOperator={perms.isOperator}
          onTune={(freqHz) => r.tuneTo({ freqHz })}
        />
      </div>
    </div>
  );
}
