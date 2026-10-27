# UI layout redesign

Status: decided, implemented in `web/src/shell/` (tuning strip, VFO tabs, inspector). Implementation
notes and tokens: `docs/ui-design.md`.

## Problems before

- Three frequency inputs: the Source centre field in the sidebar, the top bar centre readout and the
  top bar VFO readout.
- The receiver was split: frequency at the top, mode, bandwidth, squelch and tones in a dock below
  the spectrum, and the VFO list duplicated in the sidebar and in the dock chips.
- The sidebar (eight sections) scrolled; a growing bookmark list pushed Band plan, Display and Audio
  away.
- The main column scrolled and the spectrum shrank whenever the dock grew (RDS and Tones panels).

## Target layout (desktop, 900 px and wider; the page never scrolls, only panel bodies do)

- **Top bar** (h-12): logo, device switcher (driver, rate, connection dot), spacer, "Search or
  jump... Cmd+K" command palette trigger (see below), audio on/off with the
  master volume, inspector toggle, settings. No frequencies.
- **Primary tuning strip** (h-16, directly above the spectrum, full width). It is the only big
  frequency on screen.
  - Left: VFO colour dot and name, `FrequencyReadout` (digit wheel, typed entry such as `145.5m`
    plus Enter), tuning step, nudge arrows, bookmark star.
  - Middle: mode chips (WFM, NFM, AM, USB, LSB, CW), bandwidth, squelch slider with a live S-meter.
  - Right: a compact centre pill ("Centre 145.000", lock icon when locked) that opens a popover with
    the source settings (centre, rate, gain, AGC, ppm, tuning lock).
  - The strip always edits the selected VFO. With no VFO it shows an explicit "Create receiver"
    action; there is no implicit centre-tuning mode.
- **VFO tab row** under the strip: one chip per VFO (colour, number, frequency, listen/mute, close)
  plus a "+" button. It replaces the sidebar VFO list and the dock chips.
- **Bandwidth** is free: the select keeps the mode presets and "Mode default"; the pencil next to
  it types any width (`12.5`, `12.5 kHz`, `8500 Hz`), validated against the mode range and the
  widest channel the device sample rate can carry (`web/src/vfo/bandwidth.ts`; sdrd accepts any
  whole Hz in 100..1 000 000 and the channelizer must be able to build it). On the spectrum the
  selected VFO's passband edges carry grips (larger grab zone for touch); dragging them, or
  Shift+wheel over the passband (wheel up = wider), changes the bandwidth live. Alt+wheel (and
  Shift+wheel away from the passband) tunes.
- **Spectrum and waterfall** fill the remaining height (`flex-1 min-h-0`). Overlays: band ranges as a
  thin ribbon above the scale, point bookmarks as clickable pins, VFO passbands.
- **Right inspector** (w-80, collapsible, `]` toggles it, state persisted in `localStorage`) with the
  tabs Receiver, Bookmarks, Channels, Decoders, Display, Source. Every tab body scrolls on its own.
  The tab strip never scrolls on desktop: tabs show icon + label while there is room, then only
  the selected tab keeps its label (the others are icons with a tooltip), and the strip scrolls
  with arrow buttons only if even that does not fit (`web/src/shell/TabStrip.tsx`, thresholds in
  `tabLayout.ts`). Arrow keys, Home and End move between tabs (roving tabindex); icon-only tabs
  keep their text as the accessible name. The phone tab bar shows an icon above a small label.
  - Receiver: the selected VFO in detail (stereo and de-emphasis, output volume, RDS or CTCSS/DTMF,
    remove).
  - Bookmarks: the bookmark panel.
  - Channels: fixed-channel services by region, a channel grid per plan, region and "step by
    channel" settings, plan import (`docs/channel-plans.md`). The tuning strip shows a chip
    ("PMR 3") when the VFO sits on a channel; the palette resolves `pmr 3`, `dab 5c`, `ch16`.
  - Decoders: RDS and Tones (also shown inside Receiver).
  - Display: levels, zoom, floor and ceiling, band plan categories.
  - Source: centre, sample rate, gain, AGC, ppm, tuning lock, profiles, audio device.
- **Status bar** unchanged.
- No function may be dropped: the pull request that introduced this layout maps every old control to
  its new home.

## Responsive

- 600 to 900 px: the inspector is a right overlay sheet (backdrop, Esc closes); the strip wraps into
  two rows (frequency, then mode, bandwidth and squelch).
- Below 600 px: top bar with logo, palette trigger, audio and settings; the strip shows only the
  frequency; the spectrum; a bottom tab bar (Receiver, Bookmarks, Channels, Decoders, Display, Source) opens bottom
  sheets. Mode, bandwidth and squelch move into the Receiver sheet. Touch targets are at least 44 px.

## Bookmarks v2 (separate change)

- Range (start, end, label, category, colour, source) is not a bookmark (a point). Ranges never
  appear in the bookmark list.
- Bookmark: name, frequency, mode, bandwidth, squelch (dB or auto), step, tone mode (none, CTCSS,
  DCS), CTCSS Hz, DCS code and polarity, optional digital block (protocol TETRA, DMR, dPMR, P25;
  timeslot, talkgroup/GSSI, colour code, MCC/MNC, decoder plugin id), group, tags, colour, notes,
  favourite, scan settings (include, priority, dwell), created/updated.
- Groups are ordered folders (scan lists, import/export). The panel has a pinned header (search,
  group/tag filter, sort, add), a virtualised list grouped by folder, rows "name, frequency, mode,
  tone badge", click = tune the selected VFO and apply all stored fields, Alt-click = new VFO,
  double-click = edit, a "Near current frequency" section and import/export in a "..." menu.
- Migration: old bookmarks map 1:1, new fields stay empty.

## Command palette and shortcuts

- Palette (`web/src/palette/`): Ctrl/Cmd+K (also while typing) or `/` opens it; fuzzy search over a
  typed tune command ("433.92", "145.5m", "100,1 wfm", "446.00625 nfm ctcss 88.5"), bookmarks
  (name, tag, frequency), recent palette frequencies, VFOs and actions. Enter runs, Alt+Enter tunes
  in a new VFO. Modal combobox/listbox with `aria-activedescendant`, focus trapped in the field.
- One shortcut registry (`web/src/shell/shortcuts.ts`, `SHORTCUTS`) feeds the `?` overlay and the
  key hints in the palette. New keys: `F` types a frequency, `Alt+1..9` selects a VFO, `/` palette.
  Nothing fires while typing in a field or while a dialog is open.

## Multi-user: sign-in screens, roles and the admin console (P1.5)

Backend contract: `docs/api-users.md`, `docs/api-audit.md`, ADR-0011 and ADR-0012. Code:
`web/src/auth/` (gate, pages, permissions) and `web/src/admin/` (console).

### Gate (`AuthGate`)

The app is gated on `GET /api/v1/auth/status`, in this order: setup wizard, invitation page (path
`/invite/<token>`), login, forced password change, the app.

- **Login** shows "Continue as guest" below the form while `guest.enabled`. The button opens a
  card with the operator's privacy text (`guest.privacy_text`) and "Accept and continue as guest";
  `503 guests_full` and `429 too_many_guests` get plain sentences.
- **Forced password change** (`must_change_password`): one card, one-time password plus new
  password twice, no other route works until it is done. A data route that answers `403
  password_change_required` sends the gate here as well.
- **Invitation** `/invite/<token>`: previews role and inviter (`GET /auth/invites/{token}`), asks
  for user name and password, and removes the token from the address bar after accepting. A
  used, revoked or expired link says so. The page sets `referrer: same-origin`.
- **Session end**: every WebSocket close re-reads `/auth/status`. Signed out (kick, disabled
  account, reset password, guest idle timeout) returns to the login page with a notice; a changed
  role or a new forced password change is picked up without reload.

### Role-aware shell

One `Permissions` object per session (`web/src/auth/permissions.ts`, from role, kind and the guest
limits that `POST /auth/guest` returns again for a resumed guest session). Controls the role cannot
use are disabled with the reason as tooltip or hidden; the server answers 403 regardless.

| Area | admin | operator | listener | guest |
|---|---|---|---|---|
| Centre, rate, gain, ppm (Source tab, centre pill) | yes | yes | read-only | read-only |
| Tuning lock | take/release own, force-release others | take/release own | view holder | view (name hidden) |
| Profiles section | yes | yes | hidden | hidden |
| Bookmarks tab | edit | edit | read | tab hidden |
| Clear decoder history | yes | yes | no | no |
| Decoders tab | yes | yes | yes | only if the policy shows decoder events |
| Own VFOs | yes | yes | yes (quota) | yes (policy) |
| Other people's VFOs | edit and hear | edit and hear | shared: hear; others: dashed tab, no audio, read-only | same |
| Settings menu | role badge, My sessions, Change password, Admin console, Sign out | same without console | same | End guest session only |

- VFOs use the per-viewer flags `mine` and `shared` (`owner` only reaches operators and admins).
  No fixed 8-VFO limit in the UI: the "+" button is disabled only for the guest policy, everything
  else is a `403 vfo_limit` toast. The audio socket subscribes only to VFOs the viewer may hear
  (one foreign id would make the whole subscription `forbidden`).
- 403 codes become non-blocking toasts, not the red error banner: `not_owner`, `vfo_limit`,
  `mode_not_allowed`, `frequency_not_allowed`, `locked` (guest policy), `forbidden`,
  `password_change_required` (`web/src/auth/accessErrors.ts`).
- The tuning lock names its holder ("Locked by alice"; "you" for the own account). Unlock releases
  only the own lock; an admin gets "Force unlock tuning" for other accounts' locks
  (`DELETE .../lock?force=true`).
- Account menu: a pill at the far right of the top bar (avatar with initials, name, role badge;
  only the avatar below 600 px) opens Admin console (admins), My sessions, Change password and Sign
  out. The settings popover keeps the same entries at its bottom and the command palette has
  "Open the admin console" and "Show my sessions". (Until alpha.12 the entries lived only at the
  bottom of the settings popover, which nobody opened to look for an account.) "My sessions" lists
  the account's browsers (`/me/sessions`) with sign out per session; revoking the current one signs
  out.
- Admin console, Live sessions and Audit log show the client address with an address-type badge
  (Public, Private LAN, Docker gateway, CGNAT, IPv6 ...), reverse DNS, optional GeoIP location, the
  connection peer when a proxy forwarded the address, and the parsed browser. A yellow banner links
  to `docs/client-addresses.md` when every recent address is a Docker gateway.

### Admin console (`/admin`, admins only)

Own route, own chunk (lazy), no admin navigation for other roles ("Admins only" page, the API
refuses regardless). Header with "Back to receiver" and sign out; sidebar of six sections on
900 px and wider, a scrolling tab strip below that. Sections live at `/admin/<section>`:

1. **Users and invites** (`users`): accounts with role select, status badges (disabled, one-time
   password), live session count; actions: disable/enable, reset password, delete (confirmations;
   the last active admin and the own account are protected). "New user" with optional password
   (otherwise a generated one-time password is shown once, with copy). "Invite" creates a link
   with role, expiry and note, shown once with copy; the invitation list revokes open links.
2. **Live sessions** (`sessions`): every account and guest with role, address and browser as the
   audit IP mode stores them, connections, the receivers they are tuned to (frequency, mode,
   device/id), kick with confirmation; auto-refresh every 5 s (toggle).
3. **Audit log** (`audit`): filters for time range (presets or custom), actor, actor type, role,
   action (`user.*` prefix), outcome, IP or network, target; newest first, 50/100/250 rows with
   Newer/Older cursor paging, expandable rows (browser, details JSON), CSV and JSON export with
   the applied filters. Filters fold behind a button on phones.
4. **Guest access** (`guests`): enable, guests online and per address, receivers per guest, idle
   timeout, allowed modes, optional frequency window (MHz), "only while not locked", decoder
   history for guests, privacy text (4000 characters). Save bar sticks to the bottom.
5. **Audit settings** (`audit-settings`): retention for accounts and guests (0 = forever, guest
   forever is flagged), IP mode (full, truncated, none) per class, stored entries and size.
6. **Limits** (`limits`): receivers per listener and per operator (0 = daemon cap only).
7. **Client device info** (`client-info`): whose browsers report device details (off, accounts,
   accounts and guests) and the separate "Ask clients for their public IP" switch with the lookup
   service URL (https only), with the privacy warning. The reports show up in Live sessions (a
   collapsible "client-reported · unverified" block next to the server-seen address) and in the
   audit detail of `session.client_report` events (`docs/client-addresses.md`).

Settings forms keep an unsaved draft per section ("Unsaved changes", Discard). Tests: vitest for
the gate, permissions, role gating of the shell, the admin API client and every section; there is
no browser test setup in the repo yet (Playwright smoke test for criterion 8 is open).

## Later

- Mobile sheets polish and the scanner UI (the scanner backend needs an ADR first).
