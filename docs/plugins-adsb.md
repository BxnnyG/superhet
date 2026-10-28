# ADS-B plugin

`org.superhet.decoder.adsb` (`plugins/adsb`), decision record ADR-0017. An exclusive,
framed-transport plugin around `readsb` (wiedehopf's dump1090 fork, a separate GPL-3.0-or-later
process): aircraft on 1090 MHz with callsign, altitude, speed, track, position and signal level,
as a live map and table in the web UI and as events in the decoder history.

## What it does

- **Takes the whole device.** One instance claims the device at 1090 MHz and 2.4 MS/s (`cu8`;
  readsb decodes at exactly this rate). VFOs outside the window are paused while it runs, nobody
  can retune, stopping gives the previous tuning back (ARCHITECTURE 3.4, ADR-0015). Only one
  exclusive plugin (ADS-B or DAB) can run per device.
- **Live picture.** Every aircraft heard in the last 60 seconds, refreshed about once a second:
  ICAO address, callsign, barometric altitude (or "ground"), ground speed, track, climb rate,
  position, squawk, emitter category, signal level and message count. It is the plugin's `status`
  document; nothing of it is stored.
- **History.** `adsb.aircraft` events in the decoder history (`docs/api-decoder-events.md`):
  when an aircraft first shows a callsign or a position, when one of them first appears later,
  and then at most once per `event_interval_s` per aircraft. Sparse on purpose: a track log of
  every aircraft would be millions of rows a day.
- **Not done:** no aircraft database (registration, type, operator, route), no MLAT, no UAT
  (978 MHz), no range statistics, no recording of tracks.

## Using it

Web UI: the **ADS-B** tab (`docs/plugins-ui.md`). Operators press **Start ADS-B...**, confirm the
takeover (optionally entering the receiver position) and the map and table overlay opens.
Listeners and guests (per policy) see the same view read-only. API:

```
POST /api/v1/plugin-instances  {"plugin_id":"org.superhet.decoder.adsb","device":"dev0"}
GET  /api/v1/plugin-instances/{id}/status
```

No `freq_hz`: the manifest fixes the centre. Instance config (all optional, unknown keys refuse
to start):

| Key | Meaning |
|---|---|
| `lat`, `lon` | Receiver position in degrees (both or neither). Passed to readsb (`--lat/--lon`, lets it decode positions from a single message) and shown as the station on the map. It is part of the status document: everybody who may see decoders sees it. |
| `event_interval_s` | Shortest time between two history events of one aircraft, 5 to 86400, default 60. |

### The overlay

- **Map:** Leaflet on OpenStreetMap tiles (attribution in the corner). One symbol per aircraft
  with a position, rotated to its track and coloured by altitude (legend at the bottom left), a
  label with the callsign (or ICAO address), a trail of the last 40 positions, the receiver as a
  circle. Click an aircraft to select it. **Fit map** frames everything; the first picture is
  framed automatically, afterwards the view is yours.
- **Table:** callsign, ICAO, altitude, speed, track, signal (dBFS), seconds since last heard.
  Click a header to sort (again to reverse), type in the filter to match callsign, ICAO address or
  squawk, switch on "Only with position" to hide aircraft without a position. Click a row to
  select the aircraft: the map pans to it and the details (climb rate, squawk, category, position,
  message count) appear above the table. Aircraft that are in the table but not on the map have
  not sent a position yet (a dot next to the name).
- **Fading:** an aircraft keeps full strength for 15 s after the last message, then fades; at
  60 s it is gone from map and table.
- **Dark theme:** the tiles are darkened with a CSS filter in the dark theme.

**Tiles come through a caching proxy.** The browser never talks to the tile server: the map loads
`GET /api/v1/map/tiles/{z}/{x}/{y}.png` from `superhet-api` (same access as the map: everybody who
may see decoders), which fetches from `https://tile.openstreetmap.org` (or the server of
`SUPERHET_MAP_TILE_URL`) with the honest User-Agent `Superhet/<version> (+https://github.com/BxnnyG/superhet)`.
Why: OSM rejects browsers on self-hosted and LAN origins (HTTP 403 "Access blocked", 400 for a
Referer such as `http://127.0.0.1:8073/`), and its
[tile usage policy](https://operations.osmfoundation.org/policies/tiles/) wants one identifiable
client, a cache and at most two connections, not every viewer's browser. The proxy therefore

- validates the address (zoom 0-19, x and y inside the pyramid, `.png` only; anything else is a 400),
- caches tiles on disk for at least seven days (longer if the server's `max-age` says so) and
  serves a stale copy when the tile server cannot be reached,
- shares one upstream fetch between concurrent requests for the same tile,
- runs at most two upstream requests at a time and leaves a tile server alone for 30 s after an
  error (2 min after a 403/429),
- evicts the least recently used tiles when the cache exceeds its cap.

Settings of `superhet-api`: `SUPERHET_MAP_TILE_CACHE_DIR` (default `/data/tiles`, a volume in
`compose.yaml`), `SUPERHET_MAP_TILE_CACHE_MAX_MB` (default 512), `SUPERHET_MAP_TILE_MODE`
(`proxy`, the default, or `direct`). The page CSP needs no extra `img-src` origin in proxy mode.
The tile server sees the address of the Superhet server, not of the viewers; viewers' browsers
only talk to Superhet. Without internet access (and an empty cache) the aircraft are drawn on an
empty background; when the tile server refuses or cannot be reached, the map says so in a banner
instead of leaving a grid of broken tiles. The server does not work around a block: if a tile
server refuses the User-Agent above, use another tile server.

**Direct mode** (`SUPERHET_MAP_TILE_MODE=direct`) restores the old behaviour: the browser fetches
tiles from the tile server itself, the CSP `img-src` names that origin, and the map sets
`referrerpolicy="strict-origin-when-cross-origin"` on its tile images (the tile server then learns
the origin of this Superhet, never a path). Every viewer's IP address reaches the tile server and
OSM may block browsers on LAN origins; use it only for a tile server you run yourself.

**Another tile server.** Set `SUPERHET_MAP_TILE_URL` on `superhet-api` to a `{z}/{x}/{y}` template
(`http(s)` with a fixed host; `{s}` subdomains are not supported), optionally
`SUPERHET_MAP_ATTRIBUTION` (plain text) and `SUPERHET_MAP_ATTRIBUTION_URL`. The proxy fetches
from that server (in direct mode the page CSP `img-src` names its origin). A bad value is logged
and the OpenStreetMap default stays. The browser reads the source from `GET /api/v1/map-config`
(everybody who may see decoders).

### Status document (`GET .../status`, `status`)

```json
{"v":1,"mode":"running","updated_ms":1790000000000,"stale_after_s":60,
 "receiver":{"lat":51.0,"lon":7.0},"messages":4821,"messages_per_s":565.0,
 "total":3,"with_position":2,"truncated":false,"error":null,
 "aircraft":[{"icao":"3c6444","callsign":"DLH123","alt_ft":35000,"speed_kt":451.3,
   "track_deg":123.5,"vrate_fpm":-64,"lat":51.12345,"lon":6.98765,"rssi":-12.3,"squawk":"1000",
   "category":"A3","messages":812,"seen_s":0.2,"seen_pos_s":0.4}]}
```

`mode` is `starting` until readsb wrote its first file, then `running`, or `error` when readsb
died (`error` names why). Fields an aircraft has not sent are absent (`on_ground: true` replaces
the altitude on the ground). At most the freshest 400 aircraft are listed (`truncated`). `seen_s`
is the age when the document was written; the browser adds the time since it arrived.

### Events (`plugins/adsb/schema/events.json`)

`adsb.aircraft`: `icao`, `summary` (e.g. `DLH123 35000 ft 451 kt 123°`), optional `callsign`,
`alt_ft`, `on_ground`, `speed_kt`, `track_deg`, `vrate_fpm`, `lat`, `lon`, `rssi`, `squawk`,
`category`. `adsb.error`: `summary`, `message` when readsb exited.

## Reception

- Antenna: a 1090 MHz antenna makes the difference (a quarter-wave ground plane of 6.9 cm
  elements, or a collinear, outdoors); the telescopic antenna of a dongle sees only the nearest
  aircraft. A 1090 MHz bandpass filter and an LNA at the antenna help a lot.
- RTL-SDR dongles (the v5 included) tune 1090 MHz and 2.4 MS/s is within their usable bandwidth.
  A gain around 30-40 dB is a good start; the device gain comes from
  the receiver's source settings (the plugin cannot change it).
- Messages per second in the tab and the overlay tell you whether anything arrives: from tens to
  a few hundred per second with a decent antenna in a populated area, zero with the wrong gain
  or antenna.

Law: ADS-B is a public broadcast meant to be received; receiving it is legal in Germany and most
countries. The history keeps aircraft identities and positions of third parties until
`SUPERHET_EVENT_RETENTION_DAYS`; the install dialog says so.

## Development

- Adapter: `crates/superhet-plugin-adapters/src/adsb/` (`aircraft.rs` parses `aircraft.json`,
  `tracker.rs` rations events and builds the status, `mod.rs` runs readsb and the host link).
  Unit tests use the recorded files in `crates/superhet-plugin-adapters/tests/adsb/`;
  `tests/adsb_plugin.rs` runs the real adapter against a fake host and the stand-in `readsb`
  (`tests/adsb/fake-readsb`, needs `python3`).
- Image: the Dockerfile builds readsb (`READSB_COMMIT`); `scripts/check-plugin-image.sh` pipes a
  synthetic Mode S frame (`scripts/gen-adsb-iq.py`, KLM1023 / 4840D6) through it.
- Web: `web/src/adsb/` (`model.ts` parsing, ageing and sorting, `store.ts` polling,
  `AdsbPanel.tsx` the tab, `AdsbView.tsx` the overlay, `AdsbMap.tsx` Leaflet). Browser scenario:
  `web/e2e/zz-adsb.e2e.ts` (tiles answered locally).
- Manual check on hardware: with a dongle and a 1090 MHz antenna, start ADS-B from the tab and
  expect messages/s within seconds and aircraft within a minute; compare with a flight tracker for
  the same area.
