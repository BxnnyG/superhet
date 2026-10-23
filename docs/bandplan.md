# Band plan and bookmarks overlay

Status: first cut. Implementation: `web/src/bandplan/` (data, logic, settings, sidebar panel),
`web/src/spectrum/annotations.ts` (strip layout and drawing), `web/src/bookmarks/` (REST client, store,
panels, import/export).

## Built-in band plan

The plan is curated in the repository because there is no machine-readable source to import:
the Bundesnetzagentur publishes the *Frequenzplan* as PDF (and the *Allgemeinzuteilungen* as PDF
or HTML) without an API or open data download. The data lives in `web/src/bandplan/data.ts` as typed
TypeScript, one entry per range or channel plan, each with a `source` text. Anything not checked
against a primary source, anything that depends on the individual licence (business radio) and
sub-segments that other band plan editions draw differently carry `approx: true`; the overlay draws
those bars fainter and the tooltip says "Approximate".

Coverage (Germany / IARU Region 1):

| Category | Content |
| --- | --- |
| Broadcast | FM 87.5-108 MHz, DAB+ Band III 174-240 MHz with blocks 5A-13F (ETSI EN 300 401 centre frequencies) |
| Amateur | 160 m to 10 m, 6 m, 4 m, 2 m (with coarse CW/SSB/beacon/MGM/APRS/repeater/simplex/satellite segments), 70 cm (weak signal, FM simplex, satellite) and 23 cm; calling channels 145.500 and 433.500 MHz FM, 144.300 SSB, APRS 144.800 |
| PMR446 | 16 analog channels, 446.00625 MHz + n x 12.5 kHz, NFM |
| Freenet | 6 channels 149.025 / .0375 / .050 / .0875 / .100 / .1125 MHz, NFM |
| LPD433 | 69 channels from 433.075 MHz in 25 kHz steps |
| CB radio | 80 channels: 1-40 standard table (AM/FM), 41-80 on 26.565-26.955 MHz (FM) |
| SRD / ISM | 433.05-434.79 MHz (433.92 marker), 863-870 MHz |
| Aviation | VOR/ILS 108-117.975, voice 118-137 MHz, 121.5 emergency, military UHF 225-400 MHz, ADS-B 1090 MHz |
| Marine VHF / AIS | 156-162.025 MHz, channels 06, 13, 16, 70, AIS 161.975 / 162.025 MHz |
| Weather satellites | 137-138 MHz (approximate; the active satellites change) |
| BOS / TETRA | 380-385 and 390-395 MHz, range only, labelled "BOS (TETRA) - reception not permitted, § 5 TDDDG" |
| Business radio | coarse 4 m, 2 m and 70 cm ranges, all approximate, off by default |

Rules the tests enforce (`bandplan.test.ts`): ranges are sorted and unique, overlap inside a
category only as declared sub-segments (`parent`), channel math for PMR446, LPD433, Freenet, CB and
DAB, sensible visibility thresholds.

### Rendering

A strip along the top edge of the spectrum, drawn on the 2D overlay canvas (so it also works
without WebGL2):

- Thin coloured bars for ranges, stacked in up to three lanes where ranges overlap; the label
  (full name, else the short one) appears when it fits inside the visible part of the bar.
- Channel ticks with a short label below. A plan shows ticks only when its channel spacing is at
  least 4 px, labels from 22 px (`MIN_TICK_PX`, `MIN_LABEL_PX` in `bandplan/model.ts`). At 2.4 MHz
  across 1000 px PMR446 shows ticks, zoomed to a few hundred kHz it shows numbers; CB is hidden until
  zoomed in. Single markers (calling channels, 121.5, ADS-B) always carry their label.
- Bookmarks as flags in the same lane with a faint stem through the plot; their labels claim space
  before channel labels.
- The strip is as tall as it needs to be (0 when nothing is visible); the VFO chips, the window
  labels and the hover label start below it.
- Hover shows a tooltip (name, frequency, mode, bandwidth, notes, "Approximate"). A click on a
  channel with a demodulable mode, or on a bookmark, tunes the active VFO with its mode and
  bandwidth (a bookmark without a mode keeps the VFO's mode); with no VFO yet one is created.
  Ranges and non-demodulable channels (DAB, ADS-B, AIS) only show the tooltip, a click there is a
  normal spectrum click.

Sidebar section **Band plan**: master switch and one switch per category (colour swatch at the
right). Stored in `localStorage` (`superhet.bandplan.v1`, also holds the bookmark overlay switch).

### Import hook for other sources

`parseExtraBandPlan(json)` in `bandplan/model.ts` validates a version 1 document and returns ranges
and channels that `buildBandPlan(extra)` merges into the plan. Nothing wires it to the UI yet; a
later source (for example a script that converts parts of the BNetzA tables) can produce:

```
{ "v": 1, "name": "source name",
  "ranges":   [ { "name": "...", "start_hz": 0, "end_hz": 0, "category": "amateur",
                  "modes": "...", "note": "...", "approx": true, "source": "..." } ],
  "channels": [ { "name": "...", "label": "...", "freq_hz": 0, "mode": "nfm",
                  "bandwidth_hz": 12500, "category": "custom", "note": "...", "source": "..." } ] }
```

Unknown or missing categories become `custom`; invalid entries are skipped.

## Bookmarks

REST contract (implemented by the API, client in `bookmarks/api.ts`): `GET /api/v1/bookmarks`,
`POST /api/v1/bookmarks`, `PATCH` and `DELETE /api/v1/bookmarks/{id}`, `POST /api/v1/bookmarks/bulk`
with `{bookmarks, replace}`. Fields: `name`, `freq_hz`, `mode` (`wfm`/`nfm`/`am` or null),
`bandwidth_hz`, `tags[]`, `color`, `notes`. Errors are `{v, ok:false, code, error}`; the message is
shown in the dialog. After every change the list is read again.

UI:

- Sidebar **Bookmarks**: switch "Show on spectrum", "Add bookmark...", search (name, tags, notes,
  frequency), tag filter, list sorted by frequency (click tunes the active VFO or creates one; the
  bookmark at the active frequency is highlighted), edit and delete (with confirmation).
- A star next to the active VFO's readout in the top bar opens the add dialog prefilled with the
  VFO's frequency, mode and bandwidth and a name from the band plan (channel, else range).
- The add/edit dialog (`BookmarkEditorHost`, mounted once in `App`) validates name, frequency
  (`parseFrequency`: "433.5", "145.5M", ...) and bandwidth (kHz) before sending.

### Import and export

Files are parsed in the browser and sent through the bulk route; the dialog shows the number of
bookmarks found, how many are new (same frequency and name = already there), entries skipped and
entries whose modulation has no VFO mode, then offers **Merge** (only new ones) or **Replace all**.

| Format | Import | Export |
| --- | --- | --- |
| Superhet JSON (`superhet-bookmarks`, also the plain API list) | yes | yes |
| gqrx `bookmarks.csv` (`freq ; name ; modulation ; bandwidth ; tags`, tag colours) | yes | yes |
| SDR++ frequency manager JSON (`lists` / single list, mode ids 0 NFM, 1 WFM, 2 AM) | yes | no |

Superhet has the demodulators WFM, NFM, AM, USB, LSB and CW (gqrx `CW-L`/`CW-U` both map to CW).
Other modulations (DSB, raw) import without a mode and keep the original name in the notes ("Mode: USB"). SDR++ list names become tags
(the default list "General" does not).
