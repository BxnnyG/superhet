# Plugin web UI

What a user sees of the plugin system (`docs/plugin-contract.md`, `docs/api-plugins.md`,
`docs/plugins-decoders.md`). Roles follow the API: operators and admins start and stop decoders,
admins install and remove plugins; listeners and guests see the events (guests only while the guest
policy shows decoder events) and nothing of the plugin management.

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

- **Running on this VFO**: one card per instance with the state chip, binding, restarts, events,
  dropped input chunks and the last error; Stop, and Restart for a failed instance. Instances that
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
   (`CONFIG_FIELDS`): POCSAG/FLEX baud rates (checkboxes), FLEX on/off, invert, character set;
   rtl_433 protocol numbers; APRS has none. Only values that differ from the plugin's defaults are
   sent. Any other plugin gets a JSON text field (must be an object).
4. **Exclusive plugins** (whole device, e.g. ADS-B): no VFO; the dialog explains that the device is
   tuned to the plugin's frequency, the receivers outside the window are paused, nobody else can
   retune and the tuning is restored on stop, and needs the "I understand" switch. A plugin with a
   centre range asks for the centre frequency, a fixed centre is not sent.

A refused start (conflict, invalid config, unknown VFO, host down) stays in the dialog with the
server's message.

## DAB tab (`docs/plugins-dab.md`)

The tab "DAB" (operators, admins; everybody who sees the decoder history, read-only) belongs to the bundled DAB plugin. Idle: block picker and **Start DAB...**, which opens a takeover confirmation (the plugin takes the whole device, other receivers are paused, nobody can retune until it is stopped). Running: state and signal badges (SNR), the ensemble on the block, a block picker with **Tune** and **Scan all blocks**, the ensembles found (live scan results plus the history of `dab.ensemble` events, click to tune), the service list with a play button per service (operators), "Now playing" with the station text (DLS) and the slideshow image, **Enable audio** (browsers need a click) and a Listen switch for the DAB stream. A scan shows its progress with **Stop scan**. Clicking a DAB block in the channel plan opens this tab with the confirmation (or tunes the running instance). The panel follows the instance named by the plugin's claim on the device, so listeners find it without listing instances.

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

- No live push of instance state: the UI polls every 3 s.
- The manifest should carry a config schema so the form can be generated; until then
  `CONFIG_FIELDS` has to know each plugin.
- No map view for APRS positions (a link per packet only); no per-plugin cards with aggregated
  state (last reading per sensor, station list).
- The browser smoke test (`web/e2e/plugins.e2e.ts`) runs the reference plugin `example-echo`
  (python3); the first-party decoders are covered by the golden recordings of the adapter crate and
  by `web/src/decoders/render.test.ts`, which renders their expected events.
