import type { Ref } from "react";
import { cn } from "./cn";

export interface MeterTick {
  /** 0..1 along the bar. */
  at: number;
  label: string;
}

interface Props {
  /** 0..1; omit when the bar is driven imperatively through `barRef`. */
  value?: number;
  barRef?: Ref<HTMLDivElement>;
  /** Static marker position (0..1), e.g. the squelch threshold. */
  marker?: number | null;
  ticks?: readonly MeterTick[];
  "aria-label": string;
  className?: string;
}

/** Horizontal level bar with an optional scale; use `barRef` to update width outside React. */
export function Meter({
  value = 0,
  barRef,
  marker,
  ticks,
  "aria-label": ariaLabel,
  className,
}: Props) {
  return (
    <div className={cn("flex flex-col gap-1", className)}>
      <div
        role="meter"
        aria-label={ariaLabel}
        aria-valuemin={0}
        aria-valuemax={1}
        aria-valuenow={value}
        className="relative h-2 overflow-hidden rounded-full bg-surface-3"
      >
        <div
          ref={barRef}
          className="h-full rounded-full bg-accent"
          style={{ width: `${Math.min(1, Math.max(0, value)) * 100}%` }}
        />
        {marker != null && (
          <div
            aria-hidden="true"
            className="absolute inset-y-0 w-0.5 bg-fg"
            style={{ left: `${Math.min(1, Math.max(0, marker)) * 100}%` }}
          />
        )}
      </div>
      {ticks && (
        <div aria-hidden="true" className="relative h-3 text-[10px] text-fg-subtle num">
          {ticks.map((t) => (
            <span
              key={t.label}
              className="absolute -translate-x-1/2 first:translate-x-0 last:-translate-x-full"
              style={{ left: `${t.at * 100}%` }}
            >
              {t.label}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
