# ADR-0023: Per-device access policies

**Status:** accepted (2026-11-09; replaces point 5 of ADR-0020)

## Context

ADR-0011 gave the API three roles and a guest policy; ADR-0020 added several SDRs and left access rules out, so every role saw and used every device. With a second stick that is often not wanted: a private receiver that only the owner and a few accounts may use, a shared one that visitors may listen to but not retune, a stick that guests should never see. The controls have to live in the API (the daemons stay unaware of users), apply to REST and WebSockets alike, and an existing install must behave exactly as before after the upgrade.

## Decision

1. **A policy per device, stored in PostgreSQL** (`device_policies`, migration 0015; key = device id). A device without a row has the open default, so the migration adds no rows and changes nothing. Fields:
   - `view_role` (`listener`|`operator`|`admin`): lowest role that may see and listen (state, spectrum, audio, decoder output, plugin streams).
   - `control_role`: lowest role that may change anything on the device (tune, lock, VFOs, profiles, plugin instances). It sits on top of the role a route already needs; it never loosens it. The default `listener` therefore means "no extra restriction", and tuning stays operator-only as before.
   - `guests_visible`: guests (listeners without an account) may see the device.
   - `allowed_users`: when not empty, only these accounts and admins may use the device at all, guests are out; their role still decides what they may change.
   Admins always pass, so a typo cannot lock everyone out. Guests count as listeners. Account ids are plain numbers, not foreign keys: a deleted account leaves a dead id that matches nobody.
2. **Two needs, `view` and `control`.** Every route that names a device has an entry in `DEVICE_RULES` (`device_access.rs`) next to its entry in `access::RULES`: `GET` routes need `view`, everything that changes the device needs `control`. Seeing is a precondition of controlling.
3. **Fail closed, in the same shape as `RULES`.** The access middleware runs the device check right after the role check. A route with `{dev}` in its path or a WebSocket upgrade (`/api/v1/ws/*`) without a device rule is refused with `403`, and a unit test fails if a rule table and the route list disagree. The device id is read from the path or the `dev` query parameter exactly as sent; encoded, repeated or missing ids are refused with `400` instead of being decoded, so the id that was checked is the one the handler uses. If the policy table cannot be read the request is refused (`503`). Reads go through a 5 s cache that every write through the API replaces at once.
4. **Handlers that learn the device later check it themselves** with the same function: plugin instances (the device is in the stored instance, looked up at the plugin host) and the decoder history (`dev` filter, or events of devices the caller may not see are left out of the list and the export; clearing the history keeps the events of devices the caller may not control).
5. **Listings show only what the caller may use.** `GET /api/v1/devices` omits closed devices and adds `access: {view, control}` per entry, which the web UI turns into the read-only state. `GET /api/v1/plugin-instances` and `/plugin-streams` drop instances on closed devices.
6. **Answers.** A closed device is `403 device_forbidden`; a visible but read-only one answers changes with `403 device_read_only`. Refusals are audited as `access.denied` with the device. Saving or resetting a policy is audited (`device.policy_change`, `device.policy_reset`).
7. **Changes reach open sockets.** After a policy is saved the API ends the sessions that have a WebSocket on that device and no longer may see it; they reconnect and are refused. (The registry keeps the actor of each connection, so this needs no new state.)
8. **Admin API and console.** `GET /api/v1/admin/device-policies`, `PUT|DELETE /api/v1/admin/device-policies/{device}`; a "Device access" section in the admin console lists every known device with a card per device. The device picker hides closed devices; on a read-only device the whole receiver is shown with read-only permissions (no tuning, receivers, lock, profiles, plugin start).

## Consequences

- Single-device and existing installs change nothing: no rows, the open default.
- Profiles, bookmarks and channel plans are global lists, not device data; applying a profile is a change on the device and needs `control` there. The bookmark list does not say which device it was made on.
- Receivers opened before a device turned read-only stay until their owner's session ends or an admin removes them (deleting needs `control`).
- Probing: a closed device answers `403`, an unknown one gets the usual `503`/`404`, so a caller who guesses ids can tell restricted from unknown ones. The device list is the discovery surface; policies are an access control, not a secrecy feature.
- The decoder history of a device carries on being recorded by the worker for everyone; only reading it is filtered. History written before a policy was set is covered too (the filter works on the device column).
- The policy lives in the API process; the daemons and the bus still carry every device's data to every API-side subscriber. Isolating devices on the bus level (separate NATS accounts per device) is a separate, later hardening step.
- Remote devices (NATS leaf nodes) can reuse the same policies keyed by their device id.
