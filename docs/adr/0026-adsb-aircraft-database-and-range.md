# ADR-0026: ADS-B aircraft database and range statistics

**Status:** accepted (2026-11-10)

## Context

SCOPE lists an aircraft database (ICAO address to registration, type, operator) and ADR-0017
left "aircraft databases and range statistics" open. Two questions need an honest answer: which
data may Superhet use, and where do range statistics live, given that only the plugin sees every
position and that nothing of the live picture is stored (ADR-0017 item 4).

## Decision

### Aircraft database

1. **Source: the Mictronics aircraft database, Open Data Commons Attribution License 1.0**
   (`indexedDB.zip` of the Mictronics/aircraft-database repository: registration, type
   designator and flags per ICAO address, ICAO type descriptions, airline names). The repository
   carries the ODC-By text; the licence allows use and redistribution with attribution, which the
   ADS-B view gives (source and licence link under the table). Checked, not assumed:
   - *tar1090-db* (the data behind tar1090): no licence file, and it is built from the Mictronics
     data plus the ADSBexchange basic database, whose terms are unclear. Not used as a source.
   - *OpenSky aircraft database*: the page calls it "unlicensed" and out of date; OpenSky's terms
     of use otherwise limit data to non-profit research. Not used (SCOPE named it before this
     check).
   - *ADSBexchange basic-ac-db*: no licence we could verify. Not used.
2. **Superhet ships no aircraft data.** An admin presses "Download aircraft database" in the
   admin console (section ADS-B data), which shows source, licence and the exact URL first.
   The API (not the viewers' browsers) fetches that one fixed URL, size-capped, without following
   redirects, and keeps the archive under `SUPERHET_AIRCRAFT_DB_DIR` (default `/data/aircraft-db`).
   Offline installations point `SUPERHET_AIRCRAFT_DB_FILE` at a copy of the export (zip or folder
   with the JSON files) and press "Import from file". A file without the ODC-By text is imported
   but labelled "unknown licence (supplied by the operator)"; Superhet does not claim a licence it
   cannot verify.
3. **Stored in PostgreSQL** (`aircraft_db`, `aircraft_types`, `aircraft_operators`,
   `aircraft_db_meta`; migration 0016), the ICAO address as a 24-bit integer. Import replaces the
   content in one transaction (readers see the old data until commit) and runs as a background
   job with progress in `GET /api/v1/adsb/aircraft-db`; it is audited. Alternative: an in-memory
   index loaded from the archive at start-up (about 460,000 records); rejected for restart cost
   and because the database is already the place for shared state.
4. **Lookup is local.** `GET /api/v1/adsb/lookup?icao=..&operator=..` (up to 400 values) answers
   from those tables; the browser caches results and asks in debounced batches. No per-aircraft
   request leaves the server. The operator comes from the callsign's three-letter designator
   (`DLH123` -> Lufthansa): the database knows airlines, not which airline flies a given airframe
   today, and a wet-leased aircraft shows the operator of the flight number, which is what a
   listener wants.
5. **Photos are links only.** The details panel links to Planespotters (by ICAO address) and
   JetPhotos (by registration) in a new tab with `noopener noreferrer`. No image is hotlinked,
   no API or page is scraped, the page loads nothing from those sites.
6. **Country and flag from the ICAO address block** (Annex 10 Vol III, table of 24-bit address
   allocations) as a static table in the web bundle; flags are emoji (Windows draws the two
   letters, which are also written out). Symbols follow the ICAO type class from the database
   (`L2J`, `H1T`) with the emitter category as fallback.

### Range statistics

7. **The plugin computes, the worker stores.** The adapter sees every `aircraft.json` snapshot
   and knows the receiver position (config `lat`/`lon`), so it builds the maxima: farthest
   position per 5 degree bearing sector (72) and altitude band (6), per window of one minute.
   Once a minute it sends one `adsb.range` event: the minute's new maxima, the message rate, the
   aircraft count and the count with position. Positions beyond 1000 km are dropped as decoding
   errors and non-ICAO addresses (`~`, TIS-B relays) are ignored.
8. **Not decoder history.** The worker's plugin-event consumer intercepts `adsb.range` before it
   would become a `decoder_events` row and writes `adsb_range_cells` (maximum per device, sector
   and band, `GREATEST` upsert) and `adsb_activity` (one row per minute, pruned with
   `SUPERHET_EVENT_RETENTION_DAYS`). Because each event only carries its own minute, a redelivered
   or late event changes nothing and a reset really starts from zero. The EVENTS stream keeps the
   events for 7 days, so a worker outage loses nothing. Alternatives: computing in the browser
   (lost when nobody watches), a pluginhost-side store (it has the database but not the event
   semantics; the worker already stores plugin events), reading the history table (events are
   rationed to one per aircraft per minute, too sparse for maxima).
9. **A moved receiver starts new statistics**: an event whose position is more than 2 km from the
   one the stored cells belong to clears the cells first. Operators can also reset a device
   (`DELETE /api/v1/adsb/range`, audited); reset removes cells, position and activity.
10. **Served** by `GET /api/v1/adsb/range?device=&hours=` (everybody who sees decoders). The web
    view has a Range tab: polar plot (all altitudes or one band), longest reception per band,
    message rate and aircraft count history, and a coverage outline on the map.

## Consequences

- New dependency in `superhet-api`: `zip` (MIT, with `flate2`/`zlib-rs`, pure Rust) for the
  Mictronics archive; third-party notes in `docs/third-party.md`.
- The first-party ADS-B plugin sends a new event type (schema extended); plugin version 0.2.0.
  Older workers store `adsb.range` as a harmless history row, newer workers divert it.
- The database is only as good as Mictronics' crowd-sourced data (a few percent of aircraft are
  missing or outdated); the UI shows "-" and keeps the callsign.
- The server needs internet access once for the download (or a file import); viewers never do.
