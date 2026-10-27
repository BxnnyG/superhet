# Channel plans

Fixed-channel services (PMR446, Freenet, LPD433, marine VHF, CB, DAB Band III, the air band) are
one click or one keystroke away. Code: `web/src/channels/`. Server storage for imported plans:
`docs/api-channel-plans.md` (which also documents the plan JSON format).

## What ships

Versioned JSON files in `web/src/channels/plans/`, validated at startup by `parsePlan`
(`schema.ts`) and by `schema.test.ts` / `plans.test.ts`.

| Plan | Regions | Content | Licence |
| --- | --- | --- | --- |
| `pmr446` | CEPT | 16 analogue channels, 446.00625 MHz + (n-1) x 12.5 kHz, NFM. Note about dPMR446 / DMR Tier 1 | licence-free |
| `freenet-de` | DE | 6 channels: 149.0250, 149.0375, 149.0500, 149.0875, 149.1000, 149.1125 MHz, NFM | licence-free |
| `lpd433` | CEPT | 69 channels, 433.075 MHz + (n-1) x 25 kHz (to 434.775 MHz), NFM | licence-free |
| `marine-vhf` | worldwide | International table, ship transmit frequencies: channels 1-28 and 60-88, 25 kHz raster from 156.025 MHz; names for 6, 13, 15, 16, 17, 70, 75, 76 | licensed (receive freely) |
| `marine-vhf-coast` | worldwide | Coast side of the 33 duplex channels (ship + 4.600 MHz) plus AIS 87B 161.975 and 88B 162.025 MHz | licensed |
| `cb-eu` | CEPT | 40 channels, 26.965-27.405 MHz in the standard table (channels 23-25 are out of frequency order: 27.255, 27.235, 27.245), AM/FM. Marked HF | licence-free |
| `cb-de-ext` | DE | Channels 41-80, 26.565-26.955 MHz in 10 kHz steps, FM. Marked HF | licence-free |
| `dab-band3` | worldwide | 38 blocks 5A-13F, ETSI centre frequencies (174.928-239.200 MHz), mode `dab`, 1.536 MHz | receive-only |
| `airband` | worldwide | A step mode, not a channel list: 118.000-136.975 MHz as 25 kHz raster and as 8.33 kHz raster (ICAO channel names 118.005, 118.010, ...), plus the 121.500 MHz guard channel | receive-only |

DAB: which ensembles transmit on which block depends on the region and is not shipped. A scan
(DAB plugin, P2) discovers them. Selecting a block sets the device centre to the block centre and
raises the sample rate to the smallest one whose usable window (85 %) covers 1.536 MHz
(2.048 MS/s on the list in the Source tab). It needs the operator role, opens no VFO and shows
"DAB decoding needs the DAB plugin (coming in P2)".

### Sources and verification

The frequencies were written down from the following publications. **They were not re-fetched when
the files were created** (the work ran without access to the documents), so treat the tables as
reviewed-from-memory and cross-checked against each other and against the overlay copy in
`bandplan/data.ts`, not as an audit. Please re-check against the current editions before relying on
them and file an issue for any difference.

- PMR446: CEPT ERC Recommendation 70-03 Annex 1, ECC Decision (15)05 (digital and 16 analogue
  channels), BNetzA Allgemeinzuteilung PMR446.
- Freenet: BNetzA Allgemeinzuteilung fuer Freenet (149.0250-149.1125 MHz).
- LPD433: CEPT ERC Recommendation 70-03 Annex 1 (433.05-434.79 MHz), BNetzA Allgemeinzuteilung.
- Marine VHF: ITU Radio Regulations Appendix 18 (channel table, 4.600 MHz duplex spacing),
  ITU-R M.1084-5 Annex 4. Regional variants (US / Canada channel naming) are not included.
- CB: CEPT T/R 20-09 / ERC Recommendation 70-03 (27 MHz), BNetzA Allgemeinzuteilung CB-Funk
  (channels 1-80). National rules on modulation and power differ, the plan notes say so.
- DAB: ETSI EN 300 401, table of Band III block frequencies (centre frequencies).
- Air band: ICAO Annex 10 Vol. V (118-137 MHz, 25 kHz and 8.33 kHz spacing),
  Commission Implementing Regulation (EU) 1079/2012 (8.33 kHz in European airspace).

Tests that guard the data: channel counts and key frequencies per plan, coast = ship + 4.6 MHz for
every duplex channel, marine channels on the 25 kHz raster, DAB blocks ascending and 1.536 MHz
apart at least, 8.33 kHz names and frequencies, and equality with the overlay's own channel grids.

## Regions

The Channels tab has a region picker (default: the browser locale, `de-DE` gives `DE`; a bare `en`
gives none). A plan names ISO 3166-1 codes and/or groups (`EU`, `CEPT`, `ITU-R1`, `ITU-R2`,
`ITU-R3`, `WORLD`; the country lists are in `regions.ts`, deliberately not exhaustive). Plans that
apply in the region come first ("For Germany"), the rest is listed under "Other regions" and stays
fully usable. The choice is stored in `localStorage` (`superhet.channels.v1`) together with the
stepping toggle and the raster choice.

## Using channels

- **Channels tab** (inspector, after Bookmarks, also in the phone tab bar; guests see it too):
  search, region, "Step by channel" switch, service cards (name, licence badge, span, HF badge),
  then a channel grid per plan (number and MHz). Tap tunes the active VFO with the plan's mode,
  bandwidth, step and a channel's CTCSS tone; Alt-click or a 500 ms press opens a new VFO. The
  channel the VFO sits on is highlighted. Plans with a raster show a raster switch (25 / 8.33 kHz).
- **Readout chip**: when the active VFO is within tolerance of a channel the tuning strip shows a
  chip ("PMR 3", "Marine 16"). Pressed = the `<` `>` buttons and the left / right keys step
  channels in plan order (not frequency order, so CB 22 -> 23 -> 24); at the end of a plan a toast
  says so. Click the chip (or the switch in the tab) to step by kHz again. Off a channel the arrows
  step by the tuning step as before.
- **Tolerance**: a quarter of the plan's smallest channel spacing, between 0.5 and 3 kHz
  (`toleranceFor` in `lookup.ts`). Nearest channel wins, then plans of the user's region.
- **Palette** (`Ctrl+K`): a plan word and a number, with or without a space: `pmr 3`, `pmr446 3`,
  `freenet 2`, `lpd 12`, `cb 19`, `marine 16`, `coast 87b`, `dab 5c`, `dab5c`, and `ch16`
  (`ch` means marine VHF). Leading zeros and case do not matter. Enter tunes, Alt+Enter uses a new
  VFO (never for DAB). Words are the plan's `aliases`, id and chip.
- **Spectrum overlay**: the built-in services are drawn by the existing band plan strip
  (`docs/bandplan.md`; ticks appear when the zoom allows, labels at 22 px). Marine now draws the
  full table from `marine-vhf.json`. Imported custom plans appear as ticks in the "Imported"
  category.
- **Roles**: everybody tunes their own VFO (the usual per-VFO rules and guest limits apply); DAB
  blocks change the device centre and need operator or admin; importing and deleting custom plans
  needs operator or admin.

## Importing

"Import plan..." (operators) takes a plan JSON (format in `docs/api-channel-plans.md`) or a CHIRP
CSV (name, frequency, mode, CTCSS tone; the plan is named after the file and tagged with the
user's region). The browser validates the file and shows the first problem ("channels[3].freq_hz:
must be a whole number of Hz"); the server stores the result for everybody. A plan the web UI
cannot read is hidden and counted in a notice.

## Not done

- `deemphasis` is stored and documented but not applied (the VFO has no de-emphasis setting).
- No separate on/off per plan in the overlay beyond the existing categories.
- 8.33 kHz channel names beyond ICAO's three-per-block scheme, US marine channel names, FRS/GMRS,
  MURS and other regions' services are not shipped; add them as plans.
