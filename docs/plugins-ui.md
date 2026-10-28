# Plugin web UI

What a user sees of the plugin system (`docs/plugin-contract.md`, `docs/api-plugins.md`,
`docs/plugins-decoders.md`). Roles follow the API: operators and admins start and stop decoders,
admins install and remove plugins; listeners and guests see the events (guests only while the guest
policy shows decoder events) and nothing of the plugin management.

## Digital modes (mode bar, operators and admins)

The decoders are presented like OpenWebRX's "digital modes" (`web/src/digital/`). Next to the
analog mode buttons (WFM NFM AM USB LSB CW) the tuning strip has a **Digital** select with
**POCSAG**, **APRS** and **ISM** (rtl_433); a mode is listed when its plugin is installed.

- **Pick one**: the receiver is set up (NFM, the bandwidth the decoder wants: 12.5 kHz for POCSAG
  and APRS, 250 kHz for ISM; squelch off), other digital decoders on the receiver are stopped, the
  plugin starts on the receiver and the output panel appears. The first time in a browser the
  plugin's legal notice is shown and must be confirmed (the same memory as the start dialog).
- **Back to analog**: **Analog** in the same select stops the decoder; picking any other
  demodulator on the mode bar does too (a decoder is watched once its receiver was seen in NFM, so
  a plugin started by hand on a WFM receiver is not stopped by surprise). A toast says so.
- **Hint chip**: on frequencies where a digital mode usually lives the strip shows "Try APRS?"
  (144.800 / 144.390 / 145.825 MHz), "Try ISM?" (433.05 to 434.79, 868.0 to 868.6, 315, 915 MHz) or
  "Try POCSAG?" (439.9875 DAPNET, 929 to 932 MHz). It only starts the mode when clicked. The table
  is `web/src/digital/modes.ts`.
- **Output panel** (below the spectrum, collapsible, remembered): for the selected receiver's
  digital decoder a table, newest first, of what it decoded. POCSAG: time, RIC, function / type /
  baud, message. APRS: time, station, type and path, position and text with a map link. ISM: time,
  sensor model and id, channel, readings. Only events of this instance are listed. The header
  shows the state, **Settings**, **Stop** (and **Restart** after a failure).
- **Counters**: input received (bytes and level in dBFS), decoded events, time of the last one,
  dropped input chunks, rejected output lines and restarts, from the instance counters of the host
  (`docs/api-plugins.md`). Under an empty table a verdict names the likely cause: no samples have
  reached the decoder (receiver paused, device busy), the input stopped (the byte counter stands
  still between two polls), or "Listening: samples arrive, nothing decoded yet" with what to
  expect ("No packets yet on 144.800 MHz. APRS is bursty ..."). The panel polls the host every
  1.5 s.
- **Settings apply live**: the Settings button (and the Settings button of every running instance
  in the Decoders tab) edits the plugin's config in place. Changes are sent after a short pause
  with `reconfigure`: the host restarts the decoder with the new config, same instance, counters
  and history, and the status line says "Applied". POCSAG has baud rates, FLEX, signal polarity
  (default: automatic, both polarities at once), national characters; ISM the protocol numbers;
  APRS none.

The RDS card of the Decoders tab only appears for a WFM receiver on the FM broadcast band (87.5 to
108 MHz) with at least 120 kHz bandwidth, not for a WFM receiver on 433 MHz.

## Admin console: Plugins (`/admin/plugins`, admins)

- **Installed plugins**: name, version, type, SPDX licence, a "Bundled" badge for the three
  first-party decoders (the API does not report where a plugin came from; the UI knows their ids),
  "Whole device" for exclusive plugins, input and claim, homepage, number of running instances and
  the install notice (`[install] acknowledge`) in a disclosure. **Remove** asks for confirmation; the
  host refuses while an instance exists (the reason is shown).
- **Install from path**: a directory with `plugin.toml` on the machine (or container) that runs
  the plugin host (a mounted volume in the compose stack). The host validates and copies it; build
  commands are listed, never run. After the install the notice is shown.
- **Instances**: a table that refreshes every 3 s with state (Running, Starting, Restarting,
  Failed, Stopped), what it is bound to (VFO, device, frequency or "whole device"), restarts and
  recent crashes, dropped input chunks, the last error (or the input error) and **Stop** /
  **Restart** (restart only in the failed state).

## Receiver: Decoders tab (operators, admins)

The top of the tab holds "Decoders on VFO n" above the live cards (RDS, tones); the section can be
collapsed (remembered per browser). On phones the Decoders tab has its own button in the bottom bar.

- **Running on this VFO**: one card per instance with the state chip, binding, the liveness
  counters (input, decoded, last event, dropped, restarts) and the last error; Settings (live
  config editing), Stop, and Restart for a failed instance. Instances that
  take the whole device are listed under "Takes over the device".
- **Available decoders**: every installed decoder plugin that the host can run (process, simple
  transport). "Needs settings" marks a plugin whose input does not fit the selected VFO. **Start...**
  opens the start dialog.
- **State on the VFO tab**: a small plug icon in the tab of every VFO that feeds a decoder, coloured
  by the worst state (green running, amber starting/restarting, red failed), with the plugin names
  and states as its tooltip and accessible name.

### Start dialog

1. **Notice**: the manifest's licence / legal text (and link) of the plugin. The first start in a
   browser needs the "I have read this notice" switch; the confirmed text is remembered per plugin
   in `localStorage` (`superhet.plugin-notices`), a changed text is asked again. The notice is
   advisory text of the plugin author; the server does not store the confirmation.
2. **VFO settings** (manifest driven, `suggestVfo` in `web/src/plugins/model.ts`): a `discriminator`
   input needs an NFM VFO and an open squelch (bursts below the threshold would be cut), the
   manifest's `bandwidth_hz` is the channel width (rtl_433: 250 kHz). The dialog lists what differs
   and **Apply to VFO n** patches the VFO before the start; nothing is changed without the click.
3. **Config**: the manifest has no config schema, so the form is a table in the web code
   (`CONFIG_FIELDS`): POCSAG/FLEX baud rates (checkboxes), FLEX on/off, signal polarity (automatic,
   normal, inverted), character set;
   rtl_433 protocol numbers; APRS has none. Only values that differ from the plugin's defaults are
   sent. Any other plugin gets a JSON text field (must be an object).
4. **Exclusive plugins** (whole device, e.g. ADS-B): no VFO; the dialog explains that the device is
   tuned to the plugin's frequency, the receivers outside the window are paused, nobody else can
   retune and the tuning is restored on stop, and needs the "I understand" switch. A plugin with a
   centre range asks for the centre frequency, a fixed centre is not sent.

A refused start (conflict, invalid config, unknown VFO, host down) stays in the dialog with the
server's message.

## Events

The log shows plugin events next to the built-in detectors (`source` = plugin id, `kind` = event
type; unknown kinds and `vfo: null` are kept). Built-in rows stay one line; plugin rows have two
(`web/src/decoders/render.ts`):

| Kind | Headline | Second line | Details |
|---|---|---|---|
| `pocsag.message` | `RIC <address>`, function, type, baud | message | function name, raw data |
| `flex.message` | capcode (+ group size), type, baud | message | cycle, frame, phase, all capcodes |
| `aprs.packet` | callsign (object name), type, path | position + comment, message, status | symbol with a name for common ones, course/speed/altitude, raw frame, **Map** link (OpenStreetMap, opens only on click) |
| `rtl433.reading` | model `#id`, channel | temperature, humidity, pressure, wind, rain, battery ... | all readings, other fields as key/value |
| any other | the plugin's `summary` or `key=value` of the data | | all scalar data fields, raw JSON |

The chevron on a row opens the details dialog (all fields, "Raw data" JSON, Tune, Map). Clicking the
row tunes to the frequency it was heard on, as before. The **Source** select (shown once a plugin is
installed or has events) filters on the server by source; with a plugin selected the chips are its
event types, otherwise the built-in kinds. The CSV export applies the same filters and has the
`source` column. The search matches the `value` (the plugin's summary).

## Not done

- No live push of instance state: the UI polls every 3 s (1.5 s while an output panel is open).
- No DAB / ADS-B entry in the Digital select yet (those plugins do not exist); the table in
  `web/src/digital/modes.ts` is where they go.
- The Digital select is part of the tuning strip and therefore not on phones (the strip's
  demodulator controls are hidden there); phones start decoders from the Decoders tab.
- The manifest should carry a config schema so the form can be generated; until then
  `CONFIG_FIELDS` has to know each plugin.
- No map view for APRS positions (a link per packet only); no per-plugin cards with aggregated
  state (last reading per sensor, station list).
- The browser smoke test (`web/e2e/plugins.e2e.ts`) runs the reference plugin `example-echo`
  (python3); the first-party decoders are covered by the golden recordings of the adapter crate and
  by `web/src/decoders/render.test.ts`, which renders their expected events.
