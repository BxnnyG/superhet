import { AudioControls } from "../audio/AudioControls";
import { DisplayControls } from "../spectrum/DisplayControls";
import { TuningControls } from "../spectrum/TuningControls";
import { VfoPanel } from "../vfo/VfoPanel";
import { CloseIcon, IconButton, Section, cn } from "../ui";
import type { Receiver } from "./useReceiver";

interface Props {
  r: Receiver;
  /** Drawer state below the shell breakpoint; ignored on wide screens. */
  open: boolean;
  onClose: () => void;
}

export function Sidebar({ r, open, onClose }: Props) {
  return (
    <>
      <div
        aria-hidden="true"
        onClick={onClose}
        className={cn(
          "fixed inset-0 z-30 bg-overlay transition-opacity duration-150 shell:hidden",
          open ? "opacity-100" : "pointer-events-none opacity-0",
        )}
      />
      <aside
        id="sidebar"
        aria-label="Controls"
        className={cn(
          "fixed inset-y-0 left-0 z-40 flex w-80 max-w-[88vw] flex-col overflow-y-auto overflow-x-hidden border-r border-border bg-surface-1",
          "max-shell:transition-[transform,visibility] max-shell:duration-150 shell:static shell:z-auto shell:max-w-none shell:shrink-0",
          open
            ? "max-shell:translate-x-0 max-shell:shadow-pop"
            : "max-shell:invisible max-shell:-translate-x-full",
        )}
      >
        <div className="flex h-12 shrink-0 items-center justify-between border-b border-border pr-2 pl-4 shell:hidden">
          <span className="text-sm font-semibold">Controls</span>
          <IconButton label="Close controls" onClick={onClose}>
            <CloseIcon />
          </IconButton>
        </div>
        <Section title="Source">
          <TuningControls tuning={r.tuningSnap} onTune={(patch) => r.tuning.request(patch)} />
        </Section>
        <Section title="Radio">
          <VfoPanel
            vfos={r.vfoSnap}
            audio={r.audio}
            audioSnap={r.audioSnap}
            step={r.step}
            onStep={r.setStep}
            onCreate={() => r.createVfoAt(r.tuningSnap.display?.centerHz ?? null)}
            onSelect={(id) => r.vfoCtl.select(id)}
            onUpdate={(id, patch) => r.vfoCtl.update(id, patch)}
            onRemove={(id) => r.vfoCtl.remove(id)}
          />
        </Section>
        <Section title="Display">
          <DisplayControls
            controller={r.levelController}
            info={r.info}
            viewActions={r.viewActions}
          />
        </Section>
        <Section title="Audio" defaultOpen={false}>
          <AudioControls audio={r.audio} snapshot={r.audioSnap} />
        </Section>
      </aside>
    </>
  );
}
