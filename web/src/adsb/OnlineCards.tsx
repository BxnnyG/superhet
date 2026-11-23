// The online parts of the aircraft panel (ADR-0033): photo with credit, the route with progress
// along the great circle, and the notices for every state (off, loading, nothing found, a service
// that cannot be reached). Everything here is optional: the panel works with local data only.

import { useState } from "react";
import { Alert, Button, ConfirmDialog, PlaneIcon, cn } from "../ui";
import { flagEmoji } from "./icao";
import { formatDuration, minutesAt, routeProgress } from "./flightRoute";
import { formatDistance, type Tracked } from "./model";
import type { OnlineAirport, OnlinePhoto, OnlineReply, OnlineRoute, SourceState } from "./online";
import type { UnitSystem } from "./units";
import type { Load } from "./useExtras";

const link = "text-accent underline-offset-2 hover:underline";

/** Plain-language state of a source that did not deliver, or null when it did. */
export function sourceProblem(name: string, state: SourceState): string | null {
  switch (state) {
    case "unavailable":
      return `${name} could not be reached.`;
    case "rate_limited":
      return `${name} is being asked too often or refused the request; Superhet waits a few minutes.`;
    default:
      return null;
  }
}

/** The notice that takes the place of the online parts while they are off or loading. */
export function OnlineNotice({
  load,
  isAdmin,
  locked,
  onEnable,
  onRetry,
}: {
  load: Load<OnlineReply>;
  isAdmin: boolean;
  locked: boolean;
  onEnable: () => void;
  onRetry: () => void;
}) {
  if (load.status === "loading") {
    return (
      <p
        role="status"
        data-testid="adsb-online-loading"
        className="rounded-md border border-border bg-surface-2/40 p-3 text-xs text-fg-muted"
      >
        Loading photo and route…
      </p>
    );
  }
  if (load.status === "error") {
    return (
      <Alert tone="danger" className="text-xs">
        <div className="flex flex-wrap items-center gap-2">
          <span>Online details could not be loaded: {load.message}</span>
          <Button size="sm" variant="secondary" onClick={onRetry}>
            Retry
          </Button>
        </div>
      </Alert>
    );
  }
  if (load.data.enabled) return null;
  return (
    <div
      data-testid="adsb-online-off"
      className="rounded-md border border-dashed border-border p-3 text-xs text-fg-muted"
    >
      <p>
        <strong className="text-fg">Photo, route and full type name are off.</strong> They come from
        public online services, which learn which aircraft is looked at.
        {locked
          ? " The server configuration keeps them off."
          : isAdmin
            ? ""
            : " An admin can switch them on in the admin console."}
      </p>
      {isAdmin && !locked && (
        <Button size="sm" variant="secondary" className="mt-2" onClick={onEnable}>
          Enable online details
        </Button>
      )}
    </div>
  );
}

/** What enabling sends where, shown before an admin switches the sources on. */
export function EnableOnlineDialog({
  busy,
  error,
  onConfirm,
  onCancel,
}: {
  busy: boolean;
  error: string | null;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <ConfirmDialog
      title="Enable online details?"
      confirmLabel="Enable"
      busy={busy}
      onConfirm={onConfirm}
      onCancel={onCancel}
    >
      <div className="flex flex-col gap-2">
        <p>
          When somebody selects an aircraft, this server asks two public services about it. Each
          service learns the aircraft&apos;s ICAO address (and for the route its callsign) and the
          address of this server, nothing else: not the receiver position, not who is looking.
        </p>
        <ul className="list-disc pl-5">
          <li>
            <a
              className={link}
              href="https://www.adsbdb.com"
              target="_blank"
              rel="noopener noreferrer"
            >
              adsbdb.com
            </a>
            : full type name, owner, route, airline.
          </li>
          <li>
            <a
              className={link}
              href="https://www.planespotters.net"
              target="_blank"
              rel="noopener noreferrer"
            >
              Planespotters.net
            </a>
            : a photo. Each viewer&apos;s browser then loads the image from Planespotters&apos;
            image servers.
          </li>
        </ul>
        <p>You can switch each service off again in the admin console (ADS-B data).</p>
        {error !== null && <Alert tone="danger">{error}</Alert>}
      </div>
    </ConfirmDialog>
  );
}

/** The photo with the photographer's credit; the thumbnail leads back to the photo page. */
export function PhotoCard({ photo, alt }: { photo: OnlinePhoto; alt: string }) {
  const [failed, setFailed] = useState(false);
  return (
    <figure data-testid="adsb-photo" className="m-0 flex min-w-0 flex-col gap-1">
      {failed ? (
        <p
          role="status"
          className="rounded-md border border-border bg-surface-2/40 p-3 text-xs text-fg-muted"
        >
          The photo could not be loaded. If an admin has only just enabled online details, reload
          the page once so that the browser may load images from Planespotters.
        </p>
      ) : (
        // No rel="nofollow": the photo service's terms ask for a plain link back.
        <a href={photo.link} target="_blank" rel="noopener noreferrer">
          <img
            src={photo.src}
            alt={alt}
            width={photo.width ?? undefined}
            height={photo.height ?? undefined}
            loading="lazy"
            decoding="async"
            onError={() => setFailed(true)}
            className="h-40 w-full rounded-md border border-border object-cover md:h-36"
          />
        </a>
      )}
      <figcaption className="text-[11px] text-fg-muted">
        Photo: {photo.photographer} ·{" "}
        <a className={link} href={photo.link} target="_blank" rel="noopener noreferrer">
          Planespotters.net
        </a>
      </figcaption>
    </figure>
  );
}

function AirportEnd({ a, align }: { a: OnlineAirport; align: "left" | "right" }) {
  const place = [a.city, a.country].filter(Boolean).join(", ");
  return (
    <div className={cn("min-w-0", align === "right" && "text-right")}>
      <div className="text-lg leading-tight font-semibold tabular-nums">
        {a.iata ?? a.icao ?? "?"}
        {a.countryIso !== null && (
          <span className="ml-1 text-sm font-normal" aria-hidden="true">
            {flagEmoji(a.countryIso)}
          </span>
        )}
      </div>
      <div className="truncate text-xs" title={a.name}>
        {a.name}
      </div>
      {place !== "" && <div className="truncate text-[11px] text-fg-muted">{place}</div>}
    </div>
  );
}

/** Origin to destination with the aircraft's progress, marked as database data that may be wrong. */
export function RouteCard({
  route,
  a,
  units,
}: {
  route: OnlineRoute;
  a: Tracked;
  units: UnitSystem;
}) {
  const { origin, destination } = route;
  const progress =
    a.lat !== null &&
    a.lon !== null &&
    origin.lat !== null &&
    origin.lon !== null &&
    destination.lat !== null &&
    destination.lon !== null
      ? routeProgress(
          { lat: origin.lat, lon: origin.lon },
          { lat: destination.lat, lon: destination.lon },
          { lat: a.lat, lon: a.lon },
        )
      : null;
  const eta =
    progress === null || !progress.plausible ? null : minutesAt(progress.remainingKm, a.speedKt);
  const flight = [route.airline?.name, route.callsignIata ?? route.callsign]
    .filter(Boolean)
    .join(" ");
  return (
    <section
      aria-label="Route"
      data-testid="adsb-route"
      className="rounded-md border border-border bg-surface-2/40 p-3"
    >
      {flight !== "" && <p className="mb-2 text-xs text-fg-muted">{flight}</p>}
      <div className="grid grid-cols-[1fr_auto_1fr] items-start gap-2">
        <AirportEnd a={origin} align="left" />
        <PlaneIcon className="mt-1 rotate-45 text-fg-muted" aria-hidden="true" />
        <AirportEnd a={destination} align="right" />
      </div>
      {progress !== null && (
        <div className="mt-3" data-testid="adsb-route-progress">
          <div
            role="progressbar"
            aria-label="Progress along the route"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(progress.fraction * 100)}
            className="relative h-1.5 rounded-full bg-border"
          >
            <div
              className={cn("h-full rounded-full", progress.plausible ? "bg-accent" : "bg-warn")}
              style={{ width: `${(progress.fraction * 100).toFixed(1)}%` }}
            />
          </div>
          <p className="mt-1 text-xs tabular-nums text-fg-muted">
            {progress.plausible ? (
              <>
                {formatDistance(progress.doneKm, units).text} flown ·{" "}
                {formatDistance(progress.remainingKm, units).text} to go
                {eta !== null && <> · about {formatDuration(eta)} at this speed</>}
              </>
            ) : (
              <span className="text-warn">
                The aircraft is {formatDistance(progress.offKm, units).text} from the great circle
                of this route: the database entry is probably for another flight.
              </span>
            )}
          </p>
        </div>
      )}
      <p className="mt-2 text-[11px] text-fg-muted">
        From a public database, looked up by callsign. It can be wrong: diversions, changed routes
        and reused flight numbers are not known to it.
      </p>
    </section>
  );
}
