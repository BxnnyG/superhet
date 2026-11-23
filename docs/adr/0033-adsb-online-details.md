# ADR-0033: ADS-B aircraft panel with opt-in online details

**Status:** accepted (2026-11-23)

## Context

The ADS-B view already shows what the receiver hears and what the imported aircraft database
(ADR-0026) knows. Users still left for adsbexchange or Flightradar24 to see a photo, where a flight
goes, the full type name and how it has behaved over the last minutes. Closing that gap needs data
Superhet does not have: a photo, a route per flight number and a readable type name. Every public
source for that learns which aircraft somebody looks at, and each has its own terms. Superhet is
self-hosted and private by default (ADR-0026 item 4: lookups are local), so online data must be
an explicit choice, and each source's terms decide how it may be used.

## Decision

### Sources, checked against their current terms on 2026-11-23

1. **Planespotters.net photo API (used, for the photo only).** Its terms
   (`https://www.planespotters.net/photo/api`) allow a backend to call the API, and the answer
   may be cached for 24 hours. They require: the photographer credited in text next to the image,
   the thumbnail linking to the photo page given by the API (a plain link, no `nofollow`), every URL
   of the answer used unchanged (no proxying, rewriting or hot-link bypass), the image fetched by
   the viewer's browser straight from the URL the API returns (not stored, not re-hosted), a
   descriptive User-Agent with a contact URL for server clients, and no use of photos or metadata
   for machine-learning datasets. Superhet therefore **does not proxy or cache photos**: the API
   (server side, `Superhet/<version> (+https://github.com/BxnnyG/superhet)`, answer kept 6 hours in
   memory) hands the browser the image URL and the page link unchanged together with the credit; the
   browser loads the image. The cost is a CSP exception: while the photo source is on, `img-src`
   gains `https://t.plnspttrs.net` and `https://cdn.planespotters.net` (the two hosts the API
   serves images from; `web::set_image_origins`, set from the settings at start-up and on every
   change). The server drops any photo whose URL is not on those hosts (https, no port, no
   credentials) or whose page link is not on `www.planespotters.net`, so the exception cannot
   be widened by an answer. Nothing else is allowed.
2. **adsbdb.com (used, for type name, owner, route, airline).** A free public API
   (`https://api.adsbdb.com`, README of its repository) without published terms, a key or a stated
   rate limit. What its README does state is binding on us: the aircraft data is PlaneBase's, the
   route data is the work of David J Taylor and Jim Mason and "may not be copied, published, or
   incorporated into other databases without the explicit permission" of the former, and the
   photographs are airport-data's. Consequences: lookups are on demand per aircraft and callsign
   (never a bulk download), answers are kept **in memory only** (route 6 hours, aircraft 24, a miss 1)
   and never written to the database, a file or a backup, the credit is shown in the panel and in
   the admin console, and **adsbdb's photo URLs are not used** (airport-data's terms were not
   checked, so that source stays out). The honest agent is sent, requests are rate limited and paused
   after errors. Should the author object, the source is one switch away from off.
3. **hexdb.io (left out).** Its documentation shows an API with aircraft, route and image
   endpoints but states no terms, licence, attribution or rate-limit rules; the route data is the
   same standing data as above and the origin of its photos is not named. The use could not be
   confirmed, so it is not used. It could be added if it publishes terms.
4. **Not used:** the adsbexchange and FlightRadar24 APIs (paid or without terms for this use; the
   external links stay), airport-data photos, OpenSky (ADR-0026).

### Opt-in, off by default

5. **Three switches.** `adsb_online` in the settings table: `enabled` (master), `adsbdb`,
   `planespotters`. All default to off. Admins change them in the admin console (ADS-B data, card
   "Online details") or with the "Enable online details" button in the aircraft panel, which first
   shows exactly what is sent where. Changes are audited (`settings.change`, `adsb_online`).
   `SUPERHET_ADSB_ONLINE` overrides them: `off` locks everything off, a list (`adsbdb`,
   `planespotters`, `all`) locks exactly those on; the console then shows the state as locked.
6. **What is sent:** the 24-bit ICAO address (both sources) and the callsign (adsbdb, route), both
   public ADS-B broadcasts, plus the server's own address and an agent string naming Superhet and the
   project URL. Never the receiver position, an account, a viewer address or any text from the
   user. The browser sends nothing to either service except the image request to the photo hosts
   (it sees the page's `Referrer-Policy: same-origin`, so no referrer, and the viewer's address,
   which the admin consented to by switching the photo source on). While the switches are off the
   endpoint answers `{"enabled": false}` and makes no request at all.
7. **Bounded fetching.** Fixed hosts and paths with a validated address (six hex digits) and
   callsign (2-8 letters or digits), no redirects, 4 s connect / 8 s total timeout, 256 KiB cap,
   two connections and 60 requests a minute per service, a 60 s pause after an error and 5 minutes
   after a 403 or 429. A source that fails shows as "could not be reached" or "paused"; the panel
   keeps working with local data.

### The panel

8. **Layout.** `AircraftDetails` grows: header (copy, share, follow, close), photo with credit or the
   notice for the current state, airline, full type and registration, country, the route
   (origin to destination with airport names, city and country; progress along the great circle
   from the aircraft's position, distances flown and to go, and an estimated time at the current
   ground speed), squawk meaning (7500/7600/7700 in red), three charts (altitude, ground speed, vertical
   rate), the existing value grid, "seen by this receiver", link-outs and credits. Every state has
   words: off (admin sees the enable action, others are pointed to the admin console, a locked
   state is explained), loading, error with retry, no photo, no route, source unreachable or paused.
9. **Route is marked as unreliable** ("from a public database, looked up by callsign; it can be
   wrong"). A route whose airports do not fit the aircraft's position (more than a quarter of the
   route length, at least 80 km, off the great circle, or beyond either end) is flagged "probably
   for another flight" and shows no progress.
10. **Charts from what the browser already has.** `SeriesStore` samples altitude, speed and climb
    rate of every listed aircraft every 5 s for 30 minutes (like the trails, also without a
    position). Nothing is sent or stored.
11. **Seen before (local).** `GET /api/v1/adsb/history?icao=` reads the decoder history
    (`adsb.aircraft` events): first and last time, number of entries and days, callsigns used.
    It honours per-device view access (ADR-0023). Migration 0022 adds a partial expression index on
    `meta->'data'->>'icao'` so the lookup stays instant on a large history; the history only
    reaches back as far as `SUPERHET_EVENT_RETENTION_DAYS`.
12. **Copy and share** put a text summary on the clipboard or in the share sheet. It never contains
    distance or bearing from the receiver (with the aircraft's position they reveal the receiver) or
    anything about this server.

## Consequences

- Photos need the viewer's browser to reach Planespotters' image hosts, and the page CSP names them
  while the source is on. A page loaded before an admin enables photos must be reloaded once; the
  panel says so when the image is refused.
- adsbdb's data can be wrong or stale (flight numbers are reused, routes change). The panel says
  so; the aircraft's own position is the check.
- A restart empties the in-memory answers; busy installations see a short burst of requests, which the
  rate limit smooths.
- Nothing in the unit tests talks to the real services: they run against a local stub server.
