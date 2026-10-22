import type { ReactNode } from "react";
import {
  ArrowLeftIcon,
  ArrowRightIcon,
  Badge,
  FrequencyReadout,
  IconButton,
  PauseIcon,
  Select,
} from "../ui";
import type { Vfo, VfoPatch } from "../vfo/api";
import {
  STEPS_HZ,
  VFO_PAUSED_FREE_HINT,
  VFO_PAUSED_HINT,
  formatStep,
  snapFrequency,
  stepFor,
  vfoColor,
  type StepChoice,
} from "../vfo/model";

interface Props {
  /** The active VFO the big readout tunes; null tunes the device centre instead. */
  vfo: Vfo | null;
  step: StepChoice;
  onStep: (step: StepChoice) => void;
  onVfo: (id: number, patch: VfoPatch) => void;
  /** Confirmed/optimistic device centre; null while unknown. */
  centerHz: number | null;
  onCenter: (hz: number) => void;
  /** Why the centre cannot be changed (tuning lock), or null. */
  centerLockedReason?: string | null;
  /** The tuning lock is held (explains a paused VFO). */
  locked?: boolean;
  /** Tuning lock control, shown next to the centre. */
  lock?: ReactNode;
}

/**
 * Top readout. With a VFO it is that VFO's frequency (digit tuning, step nudges, step select) and
 * the device centre is a compact secondary control. Without a VFO the big readout tunes the
 * centre and says so.
 */
export function ReceiverFrequency({
  vfo,
  step,
  onStep,
  onVfo,
  centerHz,
  onCenter,
  centerLockedReason = null,
  locked = false,
  lock,
}: Props) {
  const centre = (size: "sm" | "lg") => (
    <FrequencyReadout
      label="Centre frequency"
      size={size}
      valueHz={centerHz}
      onChange={onCenter}
      locked={centerLockedReason !== null}
      lockedReason={centerLockedReason ?? undefined}
    />
  );

  if (vfo === null) {
    return (
      <div className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1">
        {centre("lg")}
        <Badge title="No VFO yet: this readout tunes the device centre. Click the spectrum to add a VFO">
          Centre
        </Badge>
        {lock}
      </div>
    );
  }

  const stepHz = stepFor(step, vfo.mode);
  const nudge = (dir: 1 | -1) =>
    onVfo(vfo.id, { freqHz: snapFrequency(vfo.freqHz + dir * stepHz, stepHz) });
  return (
    <div className="flex flex-wrap items-center justify-center gap-x-4 gap-y-2">
      <div role="group" aria-label="Active VFO" className="flex items-center gap-2">
        <span
          aria-hidden="true"
          className="size-2.5 shrink-0 rounded-full"
          style={{ background: vfoColor(vfo.id) }}
        />
        <IconButton label="Down one step" variant="secondary" onClick={() => nudge(-1)}>
          <ArrowLeftIcon />
        </IconButton>
        <FrequencyReadout
          key={vfo.id}
          label="VFO frequency"
          valueHz={vfo.freqHz}
          onChange={(hz) => onVfo(vfo.id, { freqHz: hz })}
        />
        <IconButton label="Up one step" variant="secondary" onClick={() => nudge(1)}>
          <ArrowRightIcon />
        </IconButton>
        <Select
          aria-label="Tuning step"
          size="sm"
          className="w-32"
          value={String(step)}
          onChange={(v) => onStep(v === "auto" ? "auto" : Number(v))}
          options={[
            { value: "auto", label: `Auto (${formatStep(stepFor("auto", vfo.mode))})` },
            ...STEPS_HZ.map((s) => ({ value: String(s), label: formatStep(s) })),
          ]}
        />
        {vfo.status === "paused" && (
          <Badge tone="warn" title={locked ? VFO_PAUSED_HINT : VFO_PAUSED_FREE_HINT}>
            <PauseIcon size={10} />
            paused
          </Badge>
        )}
      </div>
      <div role="group" aria-label="Device centre" className="flex items-center gap-2">
        <span className="text-xs font-medium text-fg-subtle">Centre</span>
        {centre("sm")}
        {lock}
      </div>
    </div>
  );
}
