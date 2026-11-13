// The details of the selected aircraft: identity from the aircraft database, live values in the
// chosen units (the other unit in the tooltip), emergency squawks highlighted, distance and bearing
// from the receiver, and links out.

import { Badge, Button, CloseIcon, IconButton, cn } from "../ui";
import { compassName } from "./geo";
import { countryOf, flagEmoji, operatorCode, photoLinks, trackerLinks } from "./icao";
import type { InfoCache } from "./info";
import {
  emergencyOf,
  formatAlt,
  formatDistance,
  formatGeomAlt,
  formatMach,
  formatRssi,
  formatSpeed,
  formatTrack,
  formatVrate,
  label,
  vrateArrow,
  type Tracked,
} from "./model";
import { SYMBOL_LABEL, symbolKind } from "./symbols";
import type { Measure, UnitSystem } from "./units";

interface Field {
  name: string;
  value: string;
  /** Tooltip: the value in the other unit. */
  title?: string;
  /** Emphasised (emergency squawk). */
  alert?: boolean;
}

const fromMeasure = (name: string, m: Measure): Field => ({
  name,
  value: m.text,
  title: m.other === "" ? undefined : m.other,
});

/** Seconds since the last position, now. */
export const positionAgeS = (a: Tracked): number | null =>
  a.seenPosS === null ? null : a.seenPosS + Math.max(0, a.ageS - a.seenS);

export function AircraftDetails({
  a,
  units,
  info,
  follow,
  onFollow,
  onClose,
}: {
  a: Tracked;
  units: UnitSystem;
  info: InfoCache;
  /** Re-renders the panel when lookups arrive. */
  version: number;
  follow: boolean;
  onFollow: (on: boolean) => void;
  onClose: () => void;
}) {
  const known = info.aircraftInfo(a.icao);
  const operator = info.operatorInfo(operatorCode(a.callsign));
  const country = countryOf(a.icao);
  const kind = symbolKind({
    typeCode: known?.typeCode,
    typeClass: known?.typeClass,
    wake: known?.wake,
    category: a.category,
    military: known?.military,
  });
  const emergency = emergencyOf(a);
  const links = [...photoLinks(a.icao, known?.registration ?? null), ...trackerLinks(a)];
  const posAge = positionAgeS(a);
  const arrow = vrateArrow(a.vrateFpm);
  const vrate = formatVrate(a, units);
  const where =
    a.distKm === null || a.bearing === null
      ? null
      : `${formatDistance(a.distKm, units).text} · ${Math.round(a.bearing) % 360}° ${compassName(a.bearing)}`;

  const fields: Field[] = [
    { name: "ICAO", value: a.icao.toUpperCase() },
    { name: "Registration", value: known?.registration ?? "–" },
    {
      name: "Type",
      value:
        known === null ? "–" : [known.typeCode, known.typeDesc].filter(Boolean).join(" ") || "–",
    },
    { name: "Operator", value: operator?.name ?? "–" },
    {
      name: "Country",
      value: country === null ? "–" : `${flagEmoji(country.code)} ${country.name}`.trim(),
    },
    { name: "Symbol", value: SYMBOL_LABEL[kind] },
    fromMeasure("Altitude (baro)", formatAlt(a, units)),
    fromMeasure("Altitude (GNSS)", formatGeomAlt(a, units)),
    {
      name: "Vertical rate",
      value: a.vrateFpm === null ? "–" : `${arrow} ${vrate.text}`,
      title: vrate.other === "" ? undefined : vrate.other,
    },
    fromMeasure("Ground speed", formatSpeed(a.speedKt, units)),
    fromMeasure("Indicated speed", formatSpeed(a.iasKt, units)),
    fromMeasure("True airspeed", formatSpeed(a.tasKt, units)),
    { name: "Mach", value: formatMach(a.mach) },
    { name: "Track", value: formatTrack(a.trackDeg) },
    { name: "Heading", value: formatTrack(a.headingDeg) },
    {
      name: "Squawk",
      value: a.squawk === null ? "–" : emergency === null ? a.squawk : `${a.squawk} · ${emergency}`,
      alert: emergency !== null,
    },
    {
      name: "Distance, bearing",
      value: where ?? "–",
      title: a.distKm === null ? undefined : formatDistance(a.distKm, units).other,
    },
    {
      name: "Position",
      value: a.lat === null || a.lon === null ? "–" : `${a.lat.toFixed(4)}, ${a.lon.toFixed(4)}`,
    },
    {
      name: "Position age",
      value: posAge === null ? "–" : posAge < 1 ? "now" : `${Math.round(posAge)} s`,
    },
    { name: "Signal", value: formatRssi(a) },
    { name: "Messages", value: String(a.messages) },
    { name: "Category", value: a.category ?? "–" },
  ];

  return (
    <div
      data-testid="adsb-details"
      data-emergency={emergency !== null ? a.squawk : undefined}
      className={cn(
        "shrink-0 border-b border-border p-3",
        emergency !== null && "border-l-4 border-l-danger bg-danger-soft/40",
      )}
    >
      <div className="mb-2 flex flex-wrap items-center gap-2">
        <h3 className="text-sm font-semibold">{label(a)}</h3>
        {known?.military === true && <Badge tone="warn">military</Badge>}
        {emergency !== null && (
          <Badge tone="danger" dot pulse title={emergency}>
            Emergency {a.squawk}
          </Badge>
        )}
        <span className="flex-1" />
        <Button
          size="sm"
          variant={follow ? "primary" : "secondary"}
          aria-pressed={follow}
          disabled={a.lat === null}
          onClick={() => onFollow(!follow)}
        >
          Follow
        </Button>
        <IconButton label="Clear selection" size="sm" onClick={onClose}>
          <CloseIcon />
        </IconButton>
      </div>
      <dl className="grid grid-cols-3 gap-x-3 gap-y-1.5 text-xs">
        {fields.map((f) => (
          <div key={f.name} className={f.value === "–" ? "opacity-60" : undefined}>
            <dt className="text-[11px] text-fg-muted">{f.name}</dt>
            <dd
              title={f.title}
              className={cn("font-medium tabular-nums", f.alert && "font-semibold text-danger")}
            >
              {f.value}
            </dd>
          </div>
        ))}
      </dl>
      {links.length > 0 && (
        <p className="mt-2 text-xs text-fg-muted">
          Look up:{" "}
          {links.map((l, i) => (
            <span key={l.label}>
              {i > 0 && ", "}
              <a
                href={l.url}
                target="_blank"
                rel="noopener noreferrer"
                className="text-accent underline-offset-2 hover:underline"
              >
                {l.label}
              </a>
            </span>
          ))}{" "}
          (opens the site; nothing is loaded from it here)
        </p>
      )}
    </div>
  );
}
