// The controls in the ADS-B view's header: map settings (units, trails, labels, rings) and the
// aircraft filter (altitude, type, military, ground).

import { Badge, Button, Popover, SegmentedControl, Select, SlidersIcon, Toggle } from "../ui";
import {
  activeFilters,
  NO_FILTER,
  toggleGroup,
  type AircraftFilter,
  type GroundFilter,
} from "./filters";
import { TRAIL_MINUTES, type LabelFields, type MapPrefs, type TrailMinutes } from "./prefs";
import { KIND_GROUPS } from "./symbols";
import {
  ftPerAltitudeUnit,
  UNIT_SYSTEM_LABEL,
  UNIT_SYSTEMS,
  unitNames,
  type UnitSystem,
} from "./units";

const TRAIL_OPTIONS = TRAIL_MINUTES.map((m) => ({
  value: String(m),
  label: m === 0 ? "No trails" : `Last ${m} min`,
}));

const UNIT_OPTIONS = UNIT_SYSTEMS.map((u) => ({ value: u, label: UNIT_SYSTEM_LABEL[u] }));

export function MapSettings({
  units,
  onUnits,
  prefs,
  onPrefs,
}: {
  units: UnitSystem;
  onUnits: (u: UnitSystem) => void;
  prefs: MapPrefs;
  onPrefs: (p: MapPrefs) => void;
}) {
  const setLabel = (key: keyof LabelFields, on: boolean) =>
    onPrefs({ ...prefs, labels: { ...prefs.labels, [key]: on } });
  return (
    <Popover
      label="Map settings"
      panelClassName="w-72 p-3"
      trigger={({ props }) => (
        <Button size="sm" variant="secondary" icon={<SlidersIcon />} {...props}>
          Map
        </Button>
      )}
    >
      <div className="flex flex-col gap-3 text-sm">
        <Select
          label="Units"
          size="sm"
          value={units}
          options={UNIT_OPTIONS}
          onChange={(v) => onUnits(UNIT_SYSTEMS.find((u) => u === v) ?? "metric")}
        />
        <Select
          label="Trails"
          size="sm"
          value={String(prefs.trailMinutes)}
          options={TRAIL_OPTIONS}
          onChange={(v) =>
            onPrefs({
              ...prefs,
              trailMinutes: TRAIL_MINUTES.find((m) => String(m) === v) ?? (5 as TrailMinutes),
            })
          }
        />
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-1 text-xs font-medium text-fg-muted">Labels</legend>
          <Toggle
            checked={prefs.labels.callsign}
            onChange={(on) => setLabel("callsign", on)}
            label="Callsign"
          />
          <Toggle
            checked={prefs.labels.altitude}
            onChange={(on) => setLabel("altitude", on)}
            label="Altitude"
          />
          <Toggle
            checked={prefs.labels.speed}
            onChange={(on) => setLabel("speed", on)}
            label="Speed"
          />
          <p className="text-[11px] text-fg-muted">
            Labels appear when zoomed in and thin out where they would overlap.
          </p>
        </fieldset>
        <Toggle
          checked={prefs.rings}
          onChange={(on) => onPrefs({ ...prefs, rings: on })}
          label={`Range rings (${unitNames(units).dist})`}
        />
      </div>
    </Popover>
  );
}

const GROUND_OPTIONS: { value: GroundFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "airborne", label: "Airborne" },
  { value: "ground", label: "On ground" },
];

/** Altitude box in the chosen unit; the filter itself holds feet. */
function AltitudeInput({
  label,
  ft,
  units,
  onChange,
}: {
  label: string;
  ft: number | null;
  units: UnitSystem;
  onChange: (ft: number | null) => void;
}) {
  const per = ftPerAltitudeUnit(units);
  return (
    <label className="flex flex-1 flex-col gap-1 text-xs font-medium text-fg-muted">
      {label}
      <span className="flex items-center gap-1">
        <input
          type="number"
          inputMode="numeric"
          min={0}
          step={units === "metric" ? 100 : 1000}
          value={ft === null ? "" : Math.round(ft / per)}
          onChange={(e) => {
            const n = e.target.value.trim() === "" ? null : Number(e.target.value);
            onChange(n === null || !Number.isFinite(n) ? null : Math.max(0, n) * per);
          }}
          className="focus-ring h-8 w-full min-w-0 rounded-md border border-border bg-surface-0 px-2 text-sm font-normal text-fg tabular-nums"
        />
        <span className="font-normal">{unitNames(units).alt}</span>
      </span>
    </label>
  );
}

export function FilterPopover({
  filter,
  onFilter,
  units,
}: {
  filter: AircraftFilter;
  onFilter: (f: AircraftFilter) => void;
  units: UnitSystem;
}) {
  const active = activeFilters(filter);
  return (
    <Popover
      label="Aircraft filter"
      panelClassName="w-80 p-3"
      trigger={({ props }) => (
        <Button size="sm" variant="secondary" {...props}>
          Filter
          {active > 0 && (
            <Badge tone="accent" className="ml-1.5" title={`${active} filter(s) active`}>
              {active}
            </Badge>
          )}
        </Button>
      )}
    >
      <div className="flex flex-col gap-3 text-sm">
        <div className="flex gap-2">
          <AltitudeInput
            label="Altitude from"
            ft={filter.altMinFt}
            units={units}
            onChange={(altMinFt) => onFilter({ ...filter, altMinFt })}
          />
          <AltitudeInput
            label="to"
            ft={filter.altMaxFt}
            units={units}
            onChange={(altMaxFt) => onFilter({ ...filter, altMaxFt })}
          />
        </div>
        <fieldset className="flex flex-col gap-1.5">
          <legend className="mb-1 text-xs font-medium text-fg-muted">Aircraft types</legend>
          {KIND_GROUPS.map((g) => (
            <label key={g.id} className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={!filter.hiddenGroups.includes(g.id)}
                onChange={() => onFilter(toggleGroup(filter, g.id))}
              />
              {g.label}
            </label>
          ))}
        </fieldset>
        <Toggle
          checked={filter.militaryOnly}
          onChange={(militaryOnly) => onFilter({ ...filter, militaryOnly })}
          label="Military only"
        />
        <SegmentedControl
          aria-label="Ground traffic"
          size="sm"
          value={filter.ground}
          onChange={(ground) => onFilter({ ...filter, ground })}
          options={GROUND_OPTIONS}
        />
        <Button
          size="sm"
          variant="ghost"
          disabled={active === 0}
          onClick={() => onFilter(NO_FILTER)}
        >
          Clear filters
        </Button>
      </div>
    </Popover>
  );
}
