# ADR-0011: Users, roles and guest access

**Status:** accepted (2026-10-26)

## Context

Superhet started as a strictly single-user receiver (ADR-0008: one admin account, no anonymous mode; SCOPE listed public multi-user instances and user management as non-goals). The owner now wants to share an instance: trusted friends should be able to listen with their own VFOs, a few people should be allowed to run the receiver, and optionally the public should be able to listen without an account (the OpenWebRX use case).

This supersedes the single-user parts of ADR-0008 (single-admin unique index, "no multi-user model, roles", "no anonymous mode"). The rest of ADR-0008 (Argon2id, server-side sessions, cookie, CSRF, rate limiting, deny by default, setup token) stays in force.

The hardware reality does not change: one SDR delivers one capture window (about 2.0 MHz usable at 2.4 MS/s). All users share that window. What can be per user is the channel inside it.

## Options

- **Per-user capture windows**: impossible with one device; only possible with several devices (later, ADR-0007).
- **Everybody may do everything**: simplest, but one listener could retune the receiver under everyone else.
- **Roles with a shared window and per-client VFOs** (OpenWebRX model): the server runs a DDC per VFO (already the case: sdrd channelizer), each VFO has an owner, only operators move the window.

## Decision

**Three roles, a shared capture window, per-client VFOs, and an optional guest mode.**

Roles (a user has exactly one):

| Role | May |
|---|---|
| `admin` | Everything: users, invites, settings, plugins, guest policy, audit log, kick sessions, system page (ADR-0012) |
| `operator` | Everything a listener may, plus: tune centre frequency, sample rate, gain, PPM, bias-T, tuning lock, profiles, device settings, bookmarks, recordings, start/stop the decoder instances the admin allowed |
| `listener` | Own VFO(s) inside the current capture window (create, move, mode, bandwidth, squelch), own audio, spectrum/waterfall, decoder events view; no centre, rate or gain changes, no profiles, no plugin management |

Guest = anonymous listener without an account:
- Reached through a public page; **off by default**. The admin enables it explicitly.
- A guest holds a short-lived, opaque guest session (random id in an `HttpOnly; SameSite=Strict` cookie, no password). It carries the `listener` permissions, further reduced by the guest policy.
- Admin-configurable limits: max concurrent guests, max VFOs per guest, allowed modes (e.g. WFM/NFM/AM only), idle timeout, optional "guests only when no operator holds the tuning lock", optional allowed frequency range, decoder events hidden by default.
- Guests cannot see user names, other clients' IPs or the audit log. Guest sessions are audited (ADR-0012).

Shared window and VFOs:
- One device = one window. Operators change it; listeners and guests see the change and their VFOs follow the existing rules (inside the window they stay, outside they become `paused`, `docs/api-devices.md`).
- Every VFO gets an `owner` (user id or guest session id). Listeners and guests may only modify and delete their own VFOs; operators and admins may modify any. Audio subscriptions are checked against ownership/visibility.
- The existing **tuning lock** (ARCHITECTURE 3.4: holders list, `locked` rejections) is the coordination primitive: while a holder exists, centre/rate changes are rejected with the holder shown, as today. The holder becomes a concrete user (`kind:"user"` plus user id) instead of an anonymous flag. Admins can release any lock. "Guests only when no operator holds the lock" uses the same state.
- Per-role and per-guest VFO limits are enforced in api before a request reaches sdrd; sdrd keeps a global VFO cap as a safety net. The current sdrd cap of 8 VFOs is a P1 simplification and becomes configurable (CPU budget: about 2-3 % of a core per NFM/AM VFO, more for WFM stereo; the admin page shows the load).

Authentication:
- Local accounts (Argon2id as in ADR-0008), server-side sessions per user (users see and revoke their own sessions; admins can revoke any), rate-limited login (ADR-0008 limits stay, plus per-username throttling so one address cannot spray many accounts).
- Account creation by admin: directly (one-time password) or by **invite link** (random 192-bit token stored hashed, role fixed at invite time, expiry, single use, revocable). No open self-registration.
- First-run setup creates the first admin as before. The single-row constraint on `users` is replaced by a unique, case-insensitive username plus the rule that at least one active admin remains (the last admin cannot be deleted, disabled or demoted).
- OIDC stays P5; identities map to local users and therefore to a role (ROADMAP P5).
- The session middleware stays deny-by-default; each route declares its minimum role. The guest endpoints are the only routes reachable without a user session and exist only while guest mode is on.
- Implementation touches: migration for `users.role`, `invites`, guest policy in settings, VFO `owner` in the state document; WebSocket handshakes resolve to a principal (`user` or `guest`) whose role the handlers check.

## Consequences

- The attack surface grows by one unauthenticated entry point while guest mode is on. It is off by default, rate limited per address, bounded by limits and every guest session is audited. Operators exposing it publicly should add limits in their reverse proxy.
- A public guest mode makes the operator a service provider for third parties. Legal responsibility for what guests can hear stays with the operator (no decryption; § 5 TDDDG for non-public traffic). The guest policy (modes, frequency range, hidden decoder output) makes a lawful public configuration easy; decoder output containing personal data (POCSAG, ACARS, AIS names, TETRA SDS) is hidden from guests by default.
- Guests' IP addresses are personal data (ADR-0012).
- Cost scales with clients and VFOs. The api fan-out per device is shared (one NATS subscription, ADR-0007), but DDC work in sdrd is per VFO; limits protect the host.
- Exclusive plugin claims (ADS-B, DAB; ADR-0013) take the whole window: listener VFOs outside the claimed window pause. Only operators/admins may start a claim; the UI tells listeners why their VFOs are paused.
- Existing single-user installs migrate transparently: the existing account becomes `admin`, guest mode is off.
- "Plugin" keeps meaning process plugins (decoder, codec, lookup, ui; ADR-0003). Roles do not change the plugin trust model: plugins are installed by admins and are trusted code.
