import type { TunePatch } from "../spectrum/api";
import type { TuningController } from "../spectrum/tuning";
import type { VfoController } from "../vfo/controller";

/**
 * View of the tuning controller handed to the spectrum. A plain click on the spectrum retunes
 * the selected VFO; with no VFO at all it would move the device centre, so there it creates a
 * VFO at the clicked frequency instead (nothing happens until the VFO list has loaded). All other
 * requests, and every other member, go to the real controller.
 */
export function spectrumTuningView(
  tuning: TuningController,
  vfoCtl: Pick<VfoController, "getSnapshot">,
  createVfoAt: (hz: number) => void,
): TuningController {
  return new Proxy(tuning, {
    get(target, prop) {
      if (prop === "request") {
        return (patch: TunePatch) => {
          const snap = vfoCtl.getSnapshot();
          if (patch.centerHz !== undefined && snap.vfos.length === 0) {
            if (snap.loaded) createVfoAt(patch.centerHz);
            return;
          }
          target.request(patch);
        };
      }
      const value: unknown = Reflect.get(target, prop, target);
      return typeof value === "function" ? value.bind(target) : value;
    },
  });
}
