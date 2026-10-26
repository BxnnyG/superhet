import { usePermissions } from "../auth/context";
import {
  NOT_YOUR_VFO,
  canListenVfo,
  canModifyVfo,
  knownVfoLimit,
  ownVfoCount,
} from "../auth/permissions";
import { formatFrequency } from "../spectrum/freq";
import { CloseIcon, IconButton, PauseIcon, PlainButton, PlusIcon, cn } from "../ui";
import type { Vfo } from "../vfo/api";
import { ListenToggle } from "../vfo/controls";
import { MODE_LABEL, VFO_PAUSED_FREE_HINT, VFO_PAUSED_HINT, vfoColor } from "../vfo/model";
import type { Receiver } from "./useReceiver";

/** Who a receiver belongs to, for the tab tooltip (the owner is only known to operators). */
function ownershipHint(vfo: Vfo): string {
  if (vfo.mine === true) return "Your receiver";
  if (vfo.shared === true) return "Shared receiver";
  if (vfo.mine === false) {
    return vfo.owner?.startsWith("g:") ? "A guest's receiver" : "Another user's receiver";
  }
  return "";
}

/**
 * VFO tab row under the tuning strip: one tab per receiver (colour, number, frequency, listen
 * toggle, close) plus the add button. Selecting a tab selects the VFO the strip edits.
 */
export function VfoTabs({ r }: { r: Receiver }) {
  const { vfos, selectedId, loaded } = r.vfoSnap;
  const perms = usePermissions();
  // No fixed VFO limit: the daemon cap and the per-role quota are enforced by the server (403
  // `vfo_limit` becomes a toast). Only a limit known up front, the guest policy, disables the button.
  const limit = knownVfoLimit(perms);
  const full = limit !== null && ownVfoCount(vfos) >= limit;
  const locked = r.locks.length > 0;
  const centerHz = r.tuningSnap.display?.centerHz ?? null;
  return (
    <div
      role="group"
      aria-label="VFOs"
      className="flex h-10 shrink-0 items-center gap-2 max-phone:h-14 border-b border-border bg-surface-1 px-3 shell:px-4"
    >
      <ul className="flex min-w-0 items-center gap-1.5 overflow-x-auto" aria-label="VFO list">
        {vfos.map((v) => (
          <li key={v.id} className="shrink-0">
            <VfoTab
              vfo={v}
              selected={v.id === selectedId}
              canListen={canListenVfo(perms, v)}
              canClose={canModifyVfo(perms, v)}
              listening={r.audioSnap.levels[v.id]?.listening ?? true}
              pausedHint={locked ? VFO_PAUSED_HINT : VFO_PAUSED_FREE_HINT}
              onSelect={() => r.vfoCtl.select(v.id)}
              onListen={(id, on) => r.audio.setListening(id, on)}
              onClose={() => r.vfoCtl.remove(v.id)}
            />
          </li>
        ))}
      </ul>
      <IconButton
        label="Add VFO"
        tip={
          full
            ? `Receiver limit reached (${limit}). Close one first`
            : "New VFO at the display centre (Ctrl/Alt-click on the spectrum creates one at the pointer)"
        }
        variant="secondary"
        size="sm"
        className="max-phone:size-11"
        disabled={full || !loaded || centerHz === null}
        onClick={() => r.createVfoAt(centerHz)}
      >
        <PlusIcon />
      </IconButton>
    </div>
  );
}

function VfoTab({
  vfo,
  selected,
  canListen,
  canClose,
  listening,
  pausedHint,
  onSelect,
  onListen,
  onClose,
}: {
  vfo: Vfo;
  selected: boolean;
  /** Own and shared receivers can be heard; somebody else's cannot (listeners and guests). */
  canListen: boolean;
  /** Own receivers (and any receiver for operators and admins) can be closed. */
  canClose: boolean;
  listening: boolean;
  pausedHint: string;
  onSelect: () => void;
  onListen: (id: number, on: boolean) => void;
  onClose: () => void;
}) {
  const paused = vfo.status === "paused";
  const hint = ownershipHint(vfo);
  return (
    <div
      className={cn(
        "flex h-8 items-center rounded-full border pr-0.5 pl-1 max-phone:h-11 transition-colors duration-150",
        selected
          ? "border-accent/50 bg-accent-soft text-fg"
          : "border-border-strong bg-surface-1 text-fg-muted hover:bg-surface-2",
        paused && "italic opacity-70",
        vfo.mine === false && "border-dashed",
      )}
    >
      <PlainButton
        aria-pressed={selected}
        aria-label={`VFO ${vfo.id} ${MODE_LABEL[vfo.mode]} ${formatFrequency(vfo.freqHz)} MHz${paused ? " (paused)" : ""}`}
        title={[paused ? pausedHint : "", hint].filter(Boolean).join(" · ") || undefined}
        onClick={onSelect}
        className="inline-flex h-7 items-center gap-1.5 rounded-full px-2 text-xs font-medium max-phone:h-10"
      >
        <span
          aria-hidden="true"
          className="size-2 shrink-0 rounded-full"
          style={{ background: vfoColor(vfo.id) }}
        />
        <span className="text-fg-subtle num">{vfo.id}</span>
        <span className="font-mono num">{formatFrequency(vfo.freqHz).slice(0, -4)}</span>
        {paused && <PauseIcon size={10} />}
      </PlainButton>
      {canListen && (
        <ListenToggle
          id={vfo.id}
          listening={listening}
          onListen={onListen}
          className="max-phone:size-10"
        />
      )}
      {canClose ? (
        <IconButton
          label={`Delete VFO ${vfo.id}`}
          size="sm"
          className="max-phone:size-10"
          onClick={onClose}
        >
          <CloseIcon size={14} />
        </IconButton>
      ) : (
        <span title={NOT_YOUR_VFO} className="inline-block w-2" aria-hidden="true" />
      )}
    </div>
  );
}
