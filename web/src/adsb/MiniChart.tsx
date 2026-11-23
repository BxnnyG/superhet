// A small line chart for the aircraft panel (altitude, speed, climb rate over the last minutes).
// Plain SVG, no chart library: a few hundred points at most, redrawn about once a second.

import { cn } from "../ui";
import { CHART_H, CHART_W, chartLayout, spanText, type ChartPoint } from "./chartLayout";

/** One chart: title, latest value, the line, the range and how long it covers. */
export function MiniChart({
  title,
  points,
  nowMs,
  windowMs,
  format,
  includeZero = false,
  className,
}: {
  title: string;
  points: readonly ChartPoint[];
  nowMs: number;
  windowMs: number;
  /** Text of a value in the chosen unit. */
  format: (v: number) => string;
  /** Keep the zero line in view (climb rate). */
  includeZero?: boolean;
  className?: string;
}) {
  const chart = chartLayout(points, nowMs, windowMs, includeZero);
  return (
    <figure
      className={cn("min-w-0 rounded-md border border-border bg-surface-2/40 p-2", className)}
      data-testid={`adsb-chart-${title.toLowerCase().replace(/\s+/g, "-")}`}
    >
      <figcaption className="mb-1 flex items-baseline justify-between gap-2 text-[11px]">
        <span className="text-fg-muted">{title}</span>
        <span className="font-medium tabular-nums">
          {chart?.last == null ? "–" : format(chart.last)}
        </span>
      </figcaption>
      {chart === null ? (
        <p className="py-3 text-center text-[11px] text-fg-muted">
          Collecting data: a chart appears after a few updates.
        </p>
      ) : (
        <>
          <svg
            viewBox={`0 0 ${CHART_W} ${CHART_H}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={`${title}, last ${spanText(chart.spanMs)}: from ${format(chart.min)} to ${format(chart.max)}`}
            className="block h-16 w-full overflow-visible text-accent"
          >
            {chart.zeroY !== null && (
              <line
                x1={0}
                x2={CHART_W}
                y1={chart.zeroY}
                y2={chart.zeroY}
                stroke="currentColor"
                strokeOpacity={0.25}
                strokeDasharray="3 3"
                vectorEffect="non-scaling-stroke"
              />
            )}
            <path
              d={chart.path}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.75}
              strokeLinejoin="round"
              strokeLinecap="round"
              vectorEffect="non-scaling-stroke"
            />
          </svg>
          <div className="mt-0.5 flex justify-between gap-1 text-[10px] text-fg-muted tabular-nums">
            <span>{`−${spanText(chart.spanMs)}`}</span>
            <span className="truncate">
              {format(chart.min)} to {format(chart.max)}
            </span>
            <span>now</span>
          </div>
        </>
      )}
    </figure>
  );
}
