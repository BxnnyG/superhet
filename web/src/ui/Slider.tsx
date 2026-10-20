import { useId, type CSSProperties, type ReactNode } from "react";
import { cn } from "./cn";

/** Horizontal inset of the thumb centre from the track ends (half the thumb width). */
export const THUMB_INSET_PX = 7;

/** CSS `left` for a marker at `fraction` (0..1) along a slider track. */
export function markerLeft(fraction: number): string {
  const f = Math.min(1, Math.max(0, fraction));
  return `calc(${THUMB_INSET_PX}px + (100% - ${THUMB_INSET_PX * 2}px) * ${f})`;
}

interface Props {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number) => void;
  label: string;
  /** Hide the label row (the label stays as accessible name). */
  hideLabel?: boolean;
  /** Text shown right of the label (e.g. "-80 dBFS"); also used as aria-valuetext. */
  valueLabel?: string;
  disabled?: boolean;
  className?: string;
  title?: string;
  /** Overlay rendered on the track, e.g. a live level marker positioned with `markerLeft`. */
  overlay?: ReactNode;
}

export function Slider({
  value,
  min,
  max,
  step = 1,
  onChange,
  label,
  hideLabel,
  valueLabel,
  disabled,
  className,
  title,
  overlay,
}: Props) {
  const id = useId();
  const fraction = max > min ? Math.min(1, Math.max(0, (value - min) / (max - min))) : 0;
  const fill = `calc(${THUMB_INSET_PX}px + (100% - ${THUMB_INSET_PX * 2}px) * ${fraction})`;
  return (
    <div className={cn("flex min-w-0 flex-col", className)} title={title}>
      <div className={cn("flex items-baseline justify-between gap-2", hideLabel && "sr-only")}>
        <label htmlFor={id} className="text-xs font-medium text-fg-muted">
          {label}
        </label>
        {valueLabel !== undefined && (
          <output htmlFor={id} className="font-mono text-xs text-fg num">
            {valueLabel}
          </output>
        )}
      </div>
      <div className="relative flex items-center">
        <input
          id={id}
          type="range"
          className="range"
          min={min}
          max={max}
          step={step}
          value={value}
          disabled={disabled}
          aria-valuetext={valueLabel}
          style={{ "--fill": fill } as CSSProperties}
          onChange={(e) => onChange(Number(e.target.value))}
        />
        {overlay}
      </div>
    </div>
  );
}
