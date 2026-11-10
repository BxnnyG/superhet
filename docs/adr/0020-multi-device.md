# ADR-0020: Several local SDRs, one daemon each

**Status:** accepted (2026-11-06); point 5 superseded by ADR-0023

## Context

ADR-0007 fixed one `superhet-sdrd` per SDR and put the device id into every NATS subject (`sh.v1.dev.<dev>.*`) and API path. In practice the stack still assumed one device: the web UI hard-coded `dev0`, nothing could list the daemons that exist, and compose had no way to add a second one without editing the base file. Per-device bookkeeping (VFO ownership, locks, claims, taps, restore snapshots, plugin instances) was already keyed by the device id.

## Decision

1. **No new daemon mode.** A second stick is a second `superhet-sdrd` with its own `SUPERHET_DEVICE_ID`, a serial-selected `SUPERHET_DEVICE=rtlsdr:<serial>` and an optional `SUPERHET_DEVICE_LABEL` (display name, carried in the state document). Sticks are selected by EEPROM serial because the index depends on the plug order and each container sees every USB device.
2. **Discovery in the API, not in a registry service.** `GET /api/v1/devices` lists the ids found in the KV bucket `STATE` (`dev.*` live state and `restore.*` snapshots) and asks each with `ctl.state`: an answer is online, otherwise the stored document is shown as offline. No new bus subject, no registration protocol, no state outside the bus; a crashed daemon degrades to "offline" by itself.
3. **The browser owns the choice of device.** The receiver is keyed by the device id, so a switch remounts every per-device store and subscription instead of retargeting them (no cross-device leaks by construction). The choice is per browser (`localStorage`, `?dev=` overrides), which is also what makes users on different devices independent. A device nobody serves any more falls back to the first online one.
4. **Compose: an override file.** `deploy/compose.device2.yaml` adds `superhet-sdrd2` via `extends`, so `compose.yaml` stays untouched and `install.sh --update` keeps working.
5. **No per-device access rules yet.** Roles and the guest policy apply to all devices alike; a rule set per device is a data-model change (settings document, admin UI) and was left out. *Superseded by ADR-0023 (per-device access policies).* Remote devices (NATS leaf) stay a later phase.

## Consequences

- Single-device installs change nothing: the default id is `dev0` and the picker lists one entry.
- Taps, claims, locks and plugin instances stay per device; DAB on one stick cannot take the other.
- The device list needs the `STATE` bucket, so a device that never started is invisible; the logs of its daemon are the place to look.
- Renaming a device id orphans its restore snapshot, decoder history and plugin instances.
