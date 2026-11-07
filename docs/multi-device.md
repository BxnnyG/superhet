# Several SDRs at once

One `superhet-sdrd` serves one stick (ADR-0007). A second stick is a second daemon with its own
device id; the API discovers it, the web UI offers a device picker, and everything else (spectrum,
VFOs, audio, plugins, tuning lock) is already scoped by the device id in API paths and bus subjects
(`sh.v1.dev.<dev>.*`). Single-device installs need no change: the default id stays `dev0` and the
picker only lists what exists.

## Add a second stick (compose)

1. Give the sticks distinct EEPROM serials (identical sticks ship with the same one):
   `rtl_eeprom -d 0 -s 00000001`, unplug, replug, repeat for the other stick. `rtl_test` or the
   `superhet-sdrd` log (`RTL-SDR found ... serial = ...`) shows what is attached.
2. Select the sticks by serial, not by index (the index follows the plug order and both daemons see
   every USB device). In `.env`:

   ```
   SUPERHET_DEVICE=rtlsdr:00000001
   SUPERHET_DEVICE_LABEL=Roof antenna
   SUPERHET_DEVICE2=rtlsdr:00000002
   SUPERHET_DEVICE2_ID=dev1
   SUPERHET_DEVICE2_LABEL=Garage whip
   ```

3. Layer the override next to `compose.yaml` (it adds a `superhet-sdrd2` service and leaves the base
   file alone, so `install.sh --update` keeps working):

   ```
   curl -fsSLO https://raw.githubusercontent.com/BxnnyG/superhet/main/deploy/compose.device2.yaml
   echo 'COMPOSE_FILE=compose.yaml:compose.device2.yaml' >> .env
   docker compose up -d
   ```

   In a repository checkout, `cp deploy/compose.device2.yaml .` first. For a third stick copy the
   service block under another name with its own id.

Each daemon is independent: a stick that fails to open only stops its own container (compose
restarts it); the other keeps running. Ids must differ and are limited to `A-Za-z0-9_=-`. Do not
rename a device id later: bookmarks are global, but plugin instances, decoder history and the
restore snapshot are stored per id.

## What the picker does

`GET /api/v1/devices` (`docs/api-devices.md`) lists every device that has a state in the KV bucket
`STATE`, with `online` set while its daemon answers a `ctl.state` request and `label`, `driver`,
`tuner`, centre and rate from the state. The top-bar device pill shows the active device; with more
than one device its popover lists them with an online/offline dot. Choosing one:

- remounts the receiver, so the spectrum, waterfall, VFO list, audio, decoders, DAB/ADS-B views and
  the tuning lock all show the new device (nothing of the old one stays subscribed);
- is remembered in this browser (`localStorage` key `superhet.device`); `?dev=<id>` in the URL picks
  a device for one visit and wins over the stored choice;
- falls back to the first online device when the remembered one is not served any more (the
  fallback is not stored), which also fixes a single install whose id is not `dev0`.

Users on different devices are independent: the selection is per browser, tuning, VFO ownership,
the lock and the guest limits are kept per device by the API and the daemons. Bookmarks, profiles
and channel plans are global lists; applying one acts on the device that is selected. A profile
saved from one stick loads onto another if its settings fit.

## Plugins

Plugin instances carry a `device` (`POST /api/v1/plugin-instances`, `docs/api-plugins.md`). Taps
(`sh.v1.dev.<dev>.tap.*`) and exclusive claims (`ctl.claim` on `sh.v1.dev.<dev>`) are per device, so
DAB on one stick does not touch the other. The web UI starts instances on the selected device.

## Limits

- No per-device access rules yet: every signed-in role may use every device. The guest policy and
  roles (`docs/api-users.md`) apply to all devices alike.
- The device list shows what has a state in the bus. A daemon that never started (wrong serial) has
  nothing to list: check `docker compose logs superhet-sdrd2`.
- Remote devices on another machine (NATS leaf node) are a later phase (`docs/SCOPE.md`).
- Testing without a second stick: run the second daemon on a file device, e.g.
  `SUPERHET_DEVICE2='file:/iq/x.cu8?format=cu8&rate=2048000&freq=145500000'` (`scripts/gen-test-iq.py`).
