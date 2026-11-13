import { useId, useState } from "react";
import { describeHttpError } from "../http";
import { Alert, Button, ConfirmDialog, Select, Toggle, toasts } from "../ui";
import type { AdsbApi } from "./api";
import {
  bandLabel,
  BAND_COUNT,
  SECTOR_DEG,
  SECTORS,
  bandMax,
  compass,
  farthest,
  ringStep,
  sectorMax,
  type ActivityPoint,
  type RangeData,
} from "./range";
import type { RangeState } from "./useRange";
import { distanceUnitName, formatDistanceKm, kmPerDistanceUnit, type UnitSystem } from "./units";

export const HOURS_OPTIONS = [
  { value: "6", label: "Last 6 hours" },
  { value: "24", label: "Last 24 hours" },
  { value: "168", label: "Last 7 days" },
  { value: "720", label: "Last 30 days" },
];

const bandOptions = (units: UnitSystem) => [
  { value: "all", label: "All altitudes" },
  ...Array.from({ length: BAND_COUNT }, (_, i) => ({
    value: String(i),
    label: bandLabel(i, units),
  })),
];

const fmtKm = (km: number, units: UnitSystem): string => formatDistanceKm(km, units).text;

/** Range statistics: polar plot, altitude bands and activity history. */
export function RangePanel({
  api,
  device,
  range,
  hours,
  onHours,
  canReset,
  showOutline,
  onShowOutline,
  units = "aviation",
}: {
  /** Distances and altitude bands are written in these units. */
  units?: UnitSystem;
  api: AdsbApi;
  device: string;
  range: RangeState;
  hours: number;
  onHours: (hours: number) => void;
  /** Operators may reset the statistics. */
  canReset: boolean;
  showOutline: boolean;
  onShowOutline: (on: boolean) => void;
}) {
  const [band, setBand] = useState("all");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const data = range.data;

  const reset = async () => {
    setBusy(true);
    try {
      await api.resetRange(device);
      toasts.push({ tone: "success", title: "Range statistics reset" });
      setConfirming(false);
      range.reload();
    } catch (e) {
      toasts.push({ tone: "danger", title: "Could not reset", body: describeHttpError(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div data-testid="adsb-range" className="min-h-0 flex-1 overflow-auto p-3 text-xs">
      {range.error !== null && (
        <Alert tone="warn" className="mb-3">
          {range.error}
        </Alert>
      )}
      {data === null ? (
        range.error === null && (
          <p role="status" className="text-fg-muted">
            Loading range statistics…
          </p>
        )
      ) : (
        <Body
          data={data}
          band={band}
          onBand={setBand}
          hours={hours}
          onHours={onHours}
          showOutline={showOutline}
          onShowOutline={onShowOutline}
          units={units}
          canReset={canReset}
          onReset={() => setConfirming(true)}
        />
      )}
      {confirming && (
        <ConfirmDialog
          title="Reset range statistics?"
          confirmLabel="Reset"
          confirmVariant="danger"
          busy={busy}
          onConfirm={() => void reset()}
          onCancel={() => setConfirming(false)}
        >
          Forgets the longest receptions per direction and altitude, the receiver position they
          belong to and the message rate and aircraft count history of this receiver. They start
          from zero again with the next aircraft.
        </ConfirmDialog>
      )}
    </div>
  );
}

function Body({
  data,
  band,
  onBand,
  hours,
  onHours,
  showOutline,
  onShowOutline,
  units,
  canReset,
  onReset,
}: {
  units: UnitSystem;
  data: RangeData;
  band: string;
  onBand: (band: string) => void;
  hours: number;
  onHours: (hours: number) => void;
  showOutline: boolean;
  onShowOutline: (on: boolean) => void;
  canReset: boolean;
  onReset: () => void;
}) {
  const selected = band === "all" ? null : Number(band);
  const maxima = sectorMax(data.cells, selected);
  const best = farthest(data.cells);
  const hasCells = data.cells.length > 0;
  return (
    <div className="flex flex-col gap-4">
      {data.receiver === null && (
        <Alert tone="info">
          Range needs the receiver position: enter it in the ADS-B tab when starting the decoder.
          Message rate and aircraft counts are recorded without it.
        </Alert>
      )}
      <section aria-label="Longest receptions" className="flex flex-col gap-2">
        <div className="flex flex-wrap items-end gap-2">
          <Select
            label="Altitude"
            size="sm"
            options={bandOptions(units)}
            value={band}
            onChange={onBand}
            className="w-44"
          />
          <span className="flex-1" />
          {canReset && (
            <Button size="sm" variant="secondary" onClick={onReset}>
              Reset
            </Button>
          )}
        </div>
        {hasCells ? (
          <>
            <PolarPlot
              units={units}
              maxima={maxima}
              label={selected === null ? "all altitudes" : bandLabel(selected, units)}
            />
            <p data-testid="adsb-range-best" className="text-fg-muted">
              {best === null ? null : (
                <>
                  Farthest: <strong className="text-fg">{fmtKm(best.km, units)}</strong> towards{" "}
                  {compass(best.sector)}, {bandLabel(best.band, units)}
                  {best.icao !== "" && (
                    <>
                      {" "}
                      (<span className="font-mono uppercase">{best.icao}</span>)
                    </>
                  )}
                  .
                </>
              )}
              {data.sinceMs !== null && (
                <> Collected since {new Date(data.sinceMs).toLocaleDateString()}.</>
              )}
            </p>
            <Toggle
              checked={showOutline}
              onChange={onShowOutline}
              label="Coverage outline on the map"
            />
            <BandBars cells={data} units={units} />
          </>
        ) : (
          <p role="status" className="text-fg-muted">
            No positions recorded yet. Maxima are collected while ADS-B runs with a receiver
            position.
          </p>
        )}
      </section>
      <section aria-label="Activity" className="flex flex-col gap-2">
        <Select
          aria-label="History length"
          size="sm"
          options={HOURS_OPTIONS}
          value={String(hours)}
          onChange={(v) => onHours(Number(v))}
          className="w-44"
        />
        {data.series.length < 2 ? (
          <p role="status" className="text-fg-muted">
            Not enough history yet (one point per minute).
          </p>
        ) : (
          <>
            <LineChart
              title="Messages per second"
              points={data.series}
              value={(p) => p.msgsPerS}
              color="#818cf8"
            />
            <LineChart
              title="Aircraft"
              points={data.series}
              value={(p) => p.aircraft}
              second={(p) => p.withPosition}
              color="#38bdf8"
              secondColor="#4ade80"
              legend={["heard", "with position"]}
            />
          </>
        )}
      </section>
    </div>
  );
}

const SIZE = 240;
const CENTRE = SIZE / 2;
const RADIUS = 100;

/** Polar plot: distance from the middle is the longest reception in that direction (north up). */
function PolarPlot({
  maxima,
  label,
  units,
}: {
  maxima: readonly number[];
  label: string;
  units: UnitSystem;
}) {
  const titleId = useId();
  const top = Math.max(...maxima, 1);
  // Rings are round numbers of the chosen unit; the plot itself is drawn in kilometres.
  const perUnit = kmPerDistanceUnit(units);
  const stepUnits = ringStep(top / perUnit);
  const step = stepUnits * perUnit;
  const rings = Math.max(1, Math.ceil(top / step));
  const scale = RADIUS / (rings * step);
  const point = (sector: number, edge: number, km: number): string => {
    const bearing = ((sector + edge) * SECTOR_DEG * Math.PI) / 180;
    const r = km * scale;
    return `${(CENTRE + r * Math.sin(bearing)).toFixed(1)},${(CENTRE - r * Math.cos(bearing)).toFixed(1)}`;
  };
  // One wedge per sector, so a sector without reception shows as a gap rather than a spike.
  const wedges = maxima.flatMap((km, sector) =>
    km > 0 ? [`M${CENTRE},${CENTRE} L${point(sector, 0, km)} L${point(sector, 1, km)} Z`] : [],
  );
  const ringRadius = (i: number) => i * step * scale;
  return (
    <svg
      viewBox={`0 0 ${SIZE} ${SIZE}`}
      role="img"
      aria-labelledby={titleId}
      data-testid="adsb-polar"
      className="mx-auto w-full max-w-[18rem]"
    >
      <title id={titleId}>
        {`Longest reception per direction, ${label}. ${maxima.filter((m) => m > 0).length} of ${SECTORS} sectors have data.`}
      </title>
      {Array.from({ length: rings }, (_, i) => (
        <g key={i}>
          <circle
            cx={CENTRE}
            cy={CENTRE}
            r={ringRadius(i + 1)}
            fill="none"
            stroke="currentColor"
            className="text-border-strong"
            strokeWidth={0.6}
          />
          <text
            x={CENTRE + 2}
            y={CENTRE - ringRadius(i + 1) + 8}
            fontSize={7}
            fill="currentColor"
            className="text-fg-subtle"
          >
            {(i + 1) * stepUnits} {distanceUnitName(units)}
          </text>
        </g>
      ))}
      {[0, 90, 180, 270].map((deg) => (
        <line
          key={deg}
          x1={CENTRE}
          y1={CENTRE}
          x2={CENTRE + RADIUS * Math.sin((deg * Math.PI) / 180)}
          y2={CENTRE - RADIUS * Math.cos((deg * Math.PI) / 180)}
          stroke="currentColor"
          className="text-border"
          strokeWidth={0.6}
        />
      ))}
      <path
        d={wedges.join(" ")}
        fill="#818cf8"
        fillOpacity={0.45}
        stroke="#818cf8"
        strokeWidth={0.6}
      />
      {(
        [
          ["N", 0],
          ["E", 90],
          ["S", 180],
          ["W", 270],
        ] as const
      ).map(([name, deg]) => (
        <text
          key={name}
          x={CENTRE + (RADIUS + 11) * Math.sin((deg * Math.PI) / 180)}
          y={CENTRE - (RADIUS + 11) * Math.cos((deg * Math.PI) / 180) + 3}
          fontSize={9}
          textAnchor="middle"
          fill="currentColor"
          className="text-fg-muted"
        >
          {name}
        </text>
      ))}
    </svg>
  );
}

function BandBars({ cells, units }: { cells: RangeData; units: UnitSystem }) {
  const maxima = bandMax(cells.cells);
  const top = Math.max(...maxima, 1);
  return (
    <ul aria-label="Longest reception per altitude band" className="flex flex-col gap-1">
      {Array.from({ length: BAND_COUNT }, (_, i) => bandLabel(i, units)).map((name, i) => {
        const km = maxima[i] ?? 0;
        return (
          <li key={name} className="grid grid-cols-[8.5rem_1fr_3.5rem] items-center gap-2">
            <span className="text-fg-muted">{name}</span>
            <span className="h-2 rounded-full bg-surface-2">
              <span
                className="block h-2 rounded-full bg-accent"
                style={{ width: `${(km / top) * 100}%` }}
              />
            </span>
            <span className="text-right tabular-nums">{km > 0 ? fmtKm(km, units) : "–"}</span>
          </li>
        );
      })}
    </ul>
  );
}

function LineChart({
  title,
  points,
  value,
  second,
  color,
  secondColor,
  legend,
}: {
  title: string;
  points: readonly ActivityPoint[];
  value: (p: ActivityPoint) => number;
  second?: (p: ActivityPoint) => number;
  color: string;
  secondColor?: string;
  legend?: [string, string];
}) {
  const W = 300;
  const H = 70;
  const first = points[0]?.tsMs ?? 0;
  const last = points[points.length - 1]?.tsMs ?? first + 1;
  const top = Math.max(1, ...points.map((p) => Math.max(value(p), second?.(p) ?? 0)));
  const x = (t: number) => ((t - first) / Math.max(1, last - first)) * W;
  const y = (v: number) => H - 2 - (v / top) * (H - 6);
  const path = (get: (p: ActivityPoint) => number) =>
    points
      .map((p, i) => `${i === 0 ? "M" : "L"}${x(p.tsMs).toFixed(1)},${y(get(p)).toFixed(1)}`)
      .join(" ");
  const newest = points[points.length - 1];
  return (
    <figure className="flex flex-col gap-1">
      <figcaption className="flex items-baseline gap-2 text-fg-muted">
        <span className="font-medium text-fg">{title}</span>
        <span>
          max {Math.round(top)}
          {newest !== undefined && `, now ${Math.round(value(newest))}`}
        </span>
        {legend !== undefined && (
          <span className="ml-auto">
            <span style={{ color }}>{legend[0]}</span> /{" "}
            <span style={{ color: secondColor }}>{legend[1]}</span>
          </span>
        )}
      </figcaption>
      <svg
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label={`${title} from ${new Date(first).toLocaleString()} to ${new Date(last).toLocaleString()}`}
        className="w-full rounded-md border border-border bg-surface-0"
      >
        {second !== undefined && (
          <path d={path(second)} fill="none" stroke={secondColor} strokeWidth={1.4} />
        )}
        <path d={path(value)} fill="none" stroke={color} strokeWidth={1.4} />
      </svg>
    </figure>
  );
}
