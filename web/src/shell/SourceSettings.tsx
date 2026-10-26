import { TuningControls } from "../spectrum/TuningControls";
import type { Receiver } from "./useReceiver";
import { TuningLock } from "./TuningLock";

/**
 * Device source settings (centre, sample rate, gain, AGC, ppm) with the tuning lock. Shown in the
 * strip's centre popover and at the top of the Source tab; only one of them is mounted at a time
 * on any given screen, but both edit the same device.
 */
export function SourceSettings({ r }: { r: Receiver }) {
  return (
    <div className="flex flex-col gap-3">
      <TuningLock
        holders={r.locks}
        disabled={!r.tuningSnap.state || r.tuningSnap.lockBusy}
        onToggle={r.toggleLock}
      />
      <TuningControls tuning={r.tuningSnap} onTune={(patch) => r.tuning.request(patch)} />
    </div>
  );
}
