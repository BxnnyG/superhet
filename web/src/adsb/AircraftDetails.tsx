// The panel of the selected aircraft: photo and route when online details are on, identity from the
// local aircraft database, live values in the chosen units (the other unit in the tooltip),
// charts of the last minutes, the meaning of the squawk, "seen before" from the decoder history,
// distance and bearing from the receiver, and links out. With local data only (online details
// off, no database) it still shows everything the receiver itself knows.

import { useState } from "react";
import { usePermissions } from "../auth/context";
import { describeHttpError } from "../http";
import { Badge, Button, CloseIcon, CopyIcon, IconButton, LinkIcon, cn, toasts } from "../ui";
import type { AdsbApi } from "./api";
import { compassName } from "./geo";
import { countryOf, flagEmoji, operatorCode, photoLinks, trackerLinks } from "./icao";
import type { InfoCache } from "./info";
import { MiniChart } from "./MiniChart";
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
import {
  sourceProblem,
  EnableOnlineDialog,
  OnlineNotice,
  PhotoCard,
  RouteCard,
} from "./OnlineCards";
import type { OnlineReply, SeenHistory } from "./online";
import type { SeriesStore } from "./series";
import { SERIES_WINDOW_MS } from "./series";
import { squawkMeaning } from "./squawk";
import { summaryText, typeName } from "./summary";
import { SYMBOL_LABEL, symbolKind } from "./symbols";
import { formatAltFt, formatSpeedKt, formatVrateFpm, type Measure, type UnitSystem } from "./units";
import { forgetExtras, useHistory, useOnline, type Load } from "./useExtras";

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

const when = (ms: number): string => new Date(ms).toLocaleString();

/** "3 days ago", "5 min ago". */
export function agoText(ms: number, nowMs: number): string {
  const s = Math.max(0, Math.round((nowMs - ms) / 1000));
  if (s < 90) return "just now";
  const min = Math.round(s / 60);
  if (min < 90) return `${min} min ago`;
  const h = Math.round(min / 60);
  if (h < 48) return `${h} h ago`;
  return `${Math.round(h / 24)} days ago`;
}

function SeenCard({
  load,
  a,
  firstInViewMs,
  nowMs,
}: {
  load: Load<SeenHistory | null>;
  a: Tracked;
  firstInViewMs: number | null;
  nowMs: number;
}) {
  return (
    <section
      aria-label="Seen before"
      data-testid="adsb-seen"
      className="rounded-md border border-border bg-surface-2/40 p-3 text-xs"
    >
      <h4 className="mb-1 text-[11px] font-medium text-fg-muted">Seen by this receiver</h4>
      <p className="tabular-nums">
        Heard now (last message {a.ageS < 1 ? "just now" : `${Math.round(a.ageS)} s ago`})
        {firstInViewMs !== null && (
          <>, in this view since {new Date(firstInViewMs).toLocaleTimeString()}</>
        )}
        .
      </p>
      {load.status === "loading" && <p className="mt-1 text-fg-muted">Checking the history…</p>}
      {load.status === "error" && (
        <p className="mt-1 text-fg-muted">History unavailable: {load.message}</p>
      )}
      {load.status === "ready" && load.data === null && (
        <p className="mt-1 text-fg-muted">
          Not in the decoder history yet: either the first time this receiver hears it, or the
          history does not reach back that far.
        </p>
      )}
      {load.status === "ready" && load.data !== null && (
        <div className="mt-1 flex flex-col gap-0.5 tabular-nums">
          <p>
            First heard {when(load.data.firstMs)} ({agoText(load.data.firstMs, nowMs)}), last{" "}
            {when(load.data.lastMs)} ({agoText(load.data.lastMs, nowMs)}).
          </p>
          <p>
            {load.data.events} history {load.data.events === 1 ? "entry" : "entries"} on{" "}
            {load.data.days} {load.data.days === 1 ? "day" : "days"}.
          </p>
          {load.data.callsigns.length > 0 && (
            <p>
              Flown as {load.data.callsigns.map((c) => `${c.callsign} (${c.events})`).join(", ")}.
            </p>
          )}
        </div>
      )}
    </section>
  );
}

/** Altitude, speed and climb rate of the last minutes, from what this browser received. */
function Charts({
  a,
  series,
  units,
  nowMs,
}: {
  a: Tracked;
  series: SeriesStore;
  units: UnitSystem;
  nowMs: number;
}) {
  const samples = series.get(a.icao);
  const alt = samples.map((s) => ({ t: s.t, v: s.onGround ? 0 : s.altFt }));
  const speed = samples.map((s) => ({ t: s.t, v: s.speedKt }));
  const vrate = samples.map((s) => ({ t: s.t, v: s.vrateFpm }));
  return (
    <div
      aria-label="Charts of the last minutes"
      role="group"
      data-testid="adsb-charts"
      className="grid grid-cols-1 gap-2 sm:grid-cols-3"
    >
      <MiniChart
        title="Altitude"
        points={alt}
        nowMs={nowMs}
        windowMs={SERIES_WINDOW_MS}
        format={(v) => formatAltFt(v, units).text}
      />
      <MiniChart
        title="Ground speed"
        points={speed}
        nowMs={nowMs}
        windowMs={SERIES_WINDOW_MS}
        format={(v) => formatSpeedKt(v, units).text}
      />
      <MiniChart
        title="Vertical rate"
        points={vrate}
        nowMs={nowMs}
        windowMs={SERIES_WINDOW_MS}
        format={(v) => formatVrateFpm(v, units).text}
        includeZero
      />
    </div>
  );
}

export function AircraftDetails({
  a,
  units,
  info,
  api,
  series,
  now,
  follow,
  onFollow,
  onClose,
}: {
  a: Tracked;
  units: UnitSystem;
  info: InfoCache;
  /** Online details and the decoder history. */
  api: AdsbApi;
  /** Altitude, speed and climb-rate history for the charts. */
  series: SeriesStore;
  /** The view's clock, ms; also re-renders the panel when lookups arrive. */
  now: number;
  follow: boolean;
  onFollow: (on: boolean) => void;
  onClose: () => void;
}) {
  const perms = usePermissions();
  const known = info.aircraftInfo(a.icao);
  const operator = info.operatorInfo(operatorCode(a.callsign));
  const country = countryOf(a.icao);
  const { load, retry } = useOnline(api, a.icao, a.callsign);
  const seen = useHistory(api, a.icao);
  const online: OnlineReply | null = load.status === "ready" ? load.data : null;
  const [enabling, setEnabling] = useState<null | { busy: boolean; error: string | null }>(null);

  const kind = symbolKind({
    typeCode: known?.typeCode,
    typeClass: known?.typeClass,
    wake: known?.wake,
    category: a.category,
    military: known?.military,
  });
  const emergency = emergencyOf(a);
  const squawk = squawkMeaning(a.squawk);
  const links = [...photoLinks(a.icao, known?.registration ?? null), ...trackerLinks(a)];
  const posAge = positionAgeS(a);
  const arrow = vrateArrow(a.vrateFpm);
  const vrate = formatVrate(a, units);
  const where =
    a.distKm === null || a.bearing === null
      ? null
      : `${formatDistance(a.distKm, units).text} · ${Math.round(a.bearing) % 360}° ${compassName(a.bearing)}`;
  const registration = known?.registration ?? online?.aircraft?.registration ?? null;
  const fullType = typeName(known, online);
  const airline = operator?.name ?? online?.route?.airline?.name ?? online?.aircraft?.owner ?? null;
  const firstInView = series.get(a.icao)[0]?.t ?? null;
  // The side column holds the photo, or the notice while the online parts are off or loading.
  const showSide = online?.photo != null || online?.enabled !== true;

  const enable = async () => {
    setEnabling({ busy: true, error: null });
    try {
      await api.setOnlineSettings({ enabled: true, adsbdb: true, planespotters: true });
      forgetExtras(api);
      setEnabling(null);
      toasts.push({ tone: "success", title: "Online details enabled" });
      retry();
    } catch (e) {
      setEnabling({ busy: false, error: describeHttpError(e) });
    }
  };

  const text = () => summaryText({ a, units, known, operator, online });
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text());
      toasts.push({ tone: "success", title: "Aircraft details copied" });
    } catch {
      toasts.push({
        tone: "warn",
        title: "Could not copy",
        body: "The browser refused clipboard access.",
      });
    }
  };
  const share = async () => {
    try {
      await navigator.share({ title: label(a), text: text() });
    } catch (e) {
      // Closing the share sheet is not an error.
      if (!(e instanceof DOMException && e.name === "AbortError")) {
        toasts.push({ tone: "warn", title: "Could not share" });
      }
    }
  };

  const fields: Field[] = [
    { name: "ICAO", value: a.icao.toUpperCase() },
    { name: "Registration", value: registration ?? "–" },
    {
      name: "Type",
      value:
        known === null && fullType === null
          ? "–"
          : (fullType ?? ([known?.typeCode, known?.typeDesc].filter(Boolean).join(" ") || "–")),
    },
    { name: "Operator", value: airline ?? "–" },
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

  const problems =
    online === null
      ? []
      : [
          online.adsbdb !== "off" ? sourceProblem("adsbdb.com", online.adsbdb) : null,
          online.planespotters !== "off"
            ? sourceProblem("Planespotters.net", online.planespotters)
            : null,
        ].filter((p): p is string => p !== null);

  return (
    <div
      data-testid="adsb-details"
      data-emergency={emergency !== null ? a.squawk : undefined}
      className={cn(
        "max-h-[min(36rem,62vh)] shrink-0 overflow-y-auto overscroll-contain border-b border-border p-3",
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
        <IconButton label="Copy details" size="sm" onClick={() => void copy()}>
          <CopyIcon />
        </IconButton>
        {typeof navigator !== "undefined" && typeof navigator.share === "function" && (
          <IconButton label="Share details" size="sm" onClick={() => void share()}>
            <LinkIcon />
          </IconButton>
        )}
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

      <div className="flex flex-col gap-3">
        <div className={cn("grid gap-3", showSide && "md:grid-cols-[minmax(0,15rem)_1fr]")}>
          {online?.photo != null ? (
            <PhotoCard
              photo={online.photo}
              alt={`${registration ?? a.icao.toUpperCase()}${fullType === null ? "" : `, ${fullType}`}`}
            />
          ) : (
            <OnlineNotice
              load={load}
              isAdmin={perms.isAdmin}
              locked={online?.locked === true}
              onEnable={() => setEnabling({ busy: false, error: null })}
              onRetry={retry}
            />
          )}
          <div className="min-w-0 self-start" data-testid="adsb-identity">
            {airline !== null && <p className="truncate text-base font-semibold">{airline}</p>}
            {(registration !== null || fullType !== null) && (
              <p className="text-sm">{[fullType, registration].filter(Boolean).join(" · ")}</p>
            )}
            {country !== null && (
              <p className="text-xs text-fg-muted">
                {flagEmoji(country.code)} {country.name}
              </p>
            )}
            {online?.enabled === true &&
              online.photo === null &&
              online.planespotters === "not_found" && (
                <p className="mt-1 text-xs text-fg-muted">
                  No photo of this aircraft on Planespotters.
                </p>
              )}
            {online?.enabled === true &&
              online.route === null &&
              a.callsign !== null &&
              online.adsbdb === "not_found" && (
                <p className="mt-1 text-xs text-fg-muted">No route known for {a.callsign}.</p>
              )}
            {problems.map((p) => (
              <p key={p} role="status" className="mt-1 text-xs text-warn">
                {p}
              </p>
            ))}
          </div>
        </div>

        {online?.route != null && <RouteCard route={online.route} a={a} units={units} />}

        {squawk !== null && a.squawk !== null && (
          <p
            data-testid="adsb-squawk"
            className={cn(
              "rounded-md px-3 py-2 text-xs",
              squawk.emergency
                ? "bg-danger-soft font-semibold text-danger"
                : "bg-surface-2/40 text-fg-muted",
            )}
          >
            Squawk {a.squawk}: {squawk.text}
          </p>
        )}

        <Charts a={a} series={series} units={units} nowMs={now} />

        <dl className="grid grid-cols-2 gap-x-3 gap-y-1.5 text-xs sm:grid-cols-3">
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

        <SeenCard load={seen} a={a} firstInViewMs={firstInView} nowMs={now} />

        {links.length > 0 && (
          <p className="text-xs text-fg-muted">
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
            (these links open the site; nothing is loaded from it here)
          </p>
        )}
        {online?.enabled === true && (
          <p className="text-[11px] text-fg-muted" data-testid="adsb-online-credit">
            Online details from{" "}
            <a
              className="text-accent underline-offset-2 hover:underline"
              href="https://www.adsbdb.com"
              target="_blank"
              rel="noopener noreferrer"
            >
              adsbdb.com
            </a>{" "}
            (route data by David J Taylor and Jim Mason, aircraft data from PlaneBase) and{" "}
            <a
              className="text-accent underline-offset-2 hover:underline"
              href="https://www.planespotters.net"
              target="_blank"
              rel="noopener noreferrer"
            >
              Planespotters.net
            </a>
            . They may be wrong or out of date.
          </p>
        )}
      </div>

      {enabling !== null && (
        <EnableOnlineDialog
          busy={enabling.busy}
          error={enabling.error}
          onConfirm={() => void enable()}
          onCancel={() => setEnabling(null)}
        />
      )}
    </div>
  );
}
