# ADR-0029: Public receiver page: share links and the embeddable player

**Status:** accepted (2026-11-12)

## Context

Superhet is a single-user receiver, but people want to show it: a link for a friend, a live
player on a club's web page. Guests (ADR-0011) already allow anonymous listening, but they are a
site-wide switch with a site-wide policy, they need cookies and they open the full application.
What is missing is a per-purpose capability: "this link, this device, these features, until this
date", that an admin can hand out and withdraw, and that can sit in an iframe on somebody else's
page without opening the rest of the server to that page.

## Decision

1. **A share link is a stored capability** (`share_links`, migration `0019`). It names one device,
   the allowed features (`waterfall`, `audio`, `decoder_events`, `plugin_audio`), what listeners
   are tuned to (**follow** the operator, or a **fixed** frequency and mode with an optional
   range the listener may retune inside), a per-link listener cap, an optional expiry, the
   station name / description / location text for the page and an allowlist of origins that may
   frame its embed page (default none). Admins manage links under
   `/api/v1/admin/share-links` (create, edit, revoke, rotate the token, delete).
2. **The token is the whole credential and is stored hashed.** 256 random bits, base64url,
   SHA-256 in the database, shown once in the reply that made it (create / rotate). The list only
   shows the last four characters. A revoked link is revived only by rotating, which also
   invalidates every copy of the old link. The token sits in the URL path of every request, so
   there is **no cookie and no session** (`/r/<token>`, `/embed/<token>`,
   `/api/v1/share/<token>/...`).
3. **A separate door, not a kind of session.** The public routes live outside the session check
   (`share::router`, like the invite and auth routes). One middleware, `share::gate`, guards each
   of them and fails closed: unknown token (404), expired or revoked (410), the route's feature
   not allowed (403), the device hidden from guests by a device policy (403, a link never widens
   ADR-0023), a cap reached (503 / 429). A route without an entry in `share::ROUTES` is never
   served. Handlers act only on the device the link names; the client chooses nothing about it.
4. **What a connection may hear is decided server side.** On a *follow* link the audio socket
   only accepts the VFOs of operators and admins (and shared, ownerless ones), never a guest's,
   a listener's or another share listener's. On a *fixed* link every audio connection gets a VFO
   of its own at the link's frequency, owned by `s:<link>:<n>`, deleted when the socket closes
   (an upkeep task also deletes strays after an API crash). Retuning is a socket message,
   honoured only inside the link's range, at most ten times a second, and audited. Plugin audio
   needs its own feature. The state document is trimmed to the band and the followed VFOs: no
   owners, locks, gains or other listeners.
5. **Caps and rate limits.** Per link and connection kind (`spectrum`, `audio`, `events`) at most
   `max_listeners` connections; a listener is a connection of the link's primary kind (audio, else
   spectrum, else events). Globally `SUPERHET_SHARE_MAX_LISTENERS` (default 100) and eight per
   client address (IPv6 per /64). Every request counts against a per-address window, and tokens
   that match no link against a stricter one. Banned addresses are refused by the existing ban
   gate (all share APIs are below `/api/`).
6. **Audit follows ADR-0012.** Share connections are audited as guest actors (`session.connect`
   / `session.disconnect`, name `share:<label>`, id `share:<link>:<n>`) with the address stored in
   the configured IP mode. Link changes are `share.link_create|update|revoke|rotate|delete`;
   refusals are `share.denied` (coalesced), retunes `share.tune`.
7. **Clickjacking.** Every HTML page keeps `frame-ancestors 'none'` and `X-Frame-Options: DENY`,
   with one exception: `/embed/<token>` of an *active* link that lists origins gets
   `frame-ancestors <those origins>` and no `X-Frame-Options` (it cannot name origins). Unknown,
   revoked, expired or origin-less embed links stay unframeable. The page `/r/<token>` is never
   framable. The embed page starts nothing on its own: the audio socket opens, and the browser
   unlocks audio, only after the visitor clicks play.
8. **Link previews** use static Open Graph tags in `index.html`; no station, link or visitor data
   ever goes into them.

## Consequences

- An admin can hand out a link without creating accounts or switching on guest mode, and take it
  back with one click; revoking and rotating disconnect everyone on it at once.
- Losing the link means rotating the token: the plaintext cannot be recovered. That is the price
  of storing only hashes.
- Fixed links cost one VFO per audio listener on the daemon (`SUPERHET_MAX_VFOS`, default 8);
  keep `max_listeners` below that or use a follow link, which costs nothing per listener.
- Decoder events and plugin audio are served to anyone holding the link. Pager text and similar
  payloads can be personal: allow them only on links you trust.
- The cap bookkeeping and the origin allowlist are per API process; a second API instance would
  need shared counters. Only one API runs today.
- Not covered: per-visitor bandwidth limits beyond the per-socket queues, and CDN or caching in
  front of `/embed/<token>`; the CSP is computed per request from the current link state.
