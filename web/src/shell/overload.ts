// Wording for the ADC overload warning (the front end clips; RDS and stereo suffer first).

import type { DeviceState } from "../spectrum/api";

export interface OverloadNotice {
  /** Short label for the chip. */
  label: string;
  /** One-line summary with the measured share. */
  summary: string;
  /** What to do, depending on the gain mode. */
  action: string;
}

/** The warning to show for a device state, or null when the ADC does not clip. */
export function overloadNotice(state: DeviceState | null): OverloadNotice | null {
  const ov = state?.overload;
  if (!state || !ov?.active) return null;
  const clipped =
    ov.clippedPercent >= 10 ? ov.clippedPercent.toFixed(0) : ov.clippedPercent.toFixed(1);
  const auto = state.gain?.mode === "auto";
  return {
    label: "ADC overload",
    summary: `ADC overload: ${clipped} % of the samples clip. Reduce the gain.`,
    action: auto
      ? "Gain is on Auto, which often over-drives the ADC in crowded bands (FM broadcast). Turn Automatic gain off and lower the gain until this warning goes away."
      : state.gain
        ? "Lower the gain until this warning goes away."
        : "Reduce the input level (attenuator or antenna).",
  };
}
