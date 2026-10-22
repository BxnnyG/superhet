// Wording of the non-blocking notices raised by the controllers.

import type { LockHolder } from "../spectrum/api";
import type { Retune } from "../vfo/api";
import { VFO_PAUSED_HINT } from "../vfo/model";
import { toasts } from "../ui";
import { lockedBy } from "../state/locks";

/** A centre/rate change (tune, click on the spectrum, profile load) hit the tuning lock. */
export function noticeLocked(holders: readonly LockHolder[], what = "centre or sample rate"): void {
  toasts.push({
    key: "locked",
    tone: "warn",
    title: "Tuning is locked",
    body:
      holders.length > 0
        ? `${lockedBy(holders)}. Unlock to change the ${what}.`
        : `Unlock to change the ${what}.`,
  });
}

/** Free-tuning outcome of a VFO change that left the usable window. */
export function noticeRetune(result: Exclude<Retune, "none">): void {
  if (result === "locked") {
    toasts.push({ key: "retune", tone: "warn", title: VFO_PAUSED_HINT });
  } else if (result === "failed") {
    toasts.push({
      key: "retune",
      tone: "danger",
      title: "Could not retune the device",
      body: "The VFO is outside the usable window and paused.",
    });
  }
}
