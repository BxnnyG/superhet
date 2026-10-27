import type { ReactNode } from "react";
import { Badge, PlugIcon, cn } from "../ui";
import type { InstanceState, PluginInstance } from "./api";
import { STATE_HINT, STATE_LABEL, STATE_TONE } from "./model";

/** State pill of an instance (Running, Restarting, Failed, ...). */
export function StateBadge({ state }: { state: InstanceState }) {
  return (
    <Badge
      tone={STATE_TONE[state]}
      dot
      pulse={state === "starting" || state === "backoff"}
      title={STATE_HINT[state]}
    >
      {STATE_LABEL[state]}
    </Badge>
  );
}

const WORST: readonly InstanceState[] = ["error", "backoff", "starting", "running", "stopped"];

/** The state that matters most when several instances share a VFO. */
export function worstState(instances: readonly PluginInstance[]): InstanceState {
  return WORST.find((s) => instances.some((i) => i.state === s)) ?? "stopped";
}

const DOT_TEXT: Record<InstanceState, string> = {
  starting: "text-warn",
  running: "text-success",
  backoff: "text-warn",
  error: "text-danger",
  stopped: "text-fg-subtle",
};

/**
 * Small plug icon for a VFO tab: colour = worst state of the decoders on that VFO. `names` are
 * the plugin names for the tooltip.
 */
export function VfoDecoderMark({
  instances,
  names,
  className,
}: {
  instances: readonly PluginInstance[];
  names: readonly string[];
  className?: string;
}) {
  if (instances.length === 0) return null;
  const state = worstState(instances);
  const label = `Decoders: ${instances
    .map((i, n) => `${names[n] ?? i.pluginId} ${STATE_LABEL[i.state].toLowerCase()}`)
    .join(", ")}`;
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className={cn("inline-flex shrink-0 items-center gap-0.5", DOT_TEXT[state], className)}
    >
      <PlugIcon size={12} />
      {instances.length > 1 && (
        <span className="text-[10px] font-medium num">{instances.length}</span>
      )}
    </span>
  );
}

/** A label/value pair in the dense lists of the plugin screens. */
export function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <span className="inline-flex items-baseline gap-1 text-xs text-fg-muted">
      <span className="text-fg-subtle">{label}</span>
      <span className="num text-fg">{children}</span>
    </span>
  );
}
