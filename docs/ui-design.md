# UI design

Status: P1. Implementation: `web/src/ui/` (kit), `web/src/shell/` (layout), tokens in `web/src/index.css`.

## Direction

A modern product UI in the spirit of NetBird, Tailscale, Linear or the Vercel dashboard. SDR#/SDR++
were references for which functions exist, not for the look. Rules:

- Calm, clean surfaces on a neutral zinc palette, one accent colour (indigo), 1px subtle borders,
  radius 8-12 px, soft shadows only on overlays (popovers, drawer, tooltips).
- Cards and sections instead of boxed default controls: pill status badges, segmented controls,
  switches, sliders with value labels, icon buttons with tooltips, designed empty states.
- Typography: Inter-style system sans stack (no font fetch), small uppercase muted section titles,
  tabular numbers for values, monospace only for frequencies and numbers.
- Light and dark are both first-class. The theme follows `prefers-color-scheme` and can be forced in
  the settings menu (system / light / dark, persisted in `localStorage`, guarded by try/catch).
  The spectrum/waterfall area stays dark in both themes.
- Subtle 150 ms transitions (disabled for `prefers-reduced-motion`), visible focus rings
  (`focus-ring` utility), no clutter. No feature is hidden for looks: everything stays reachable.

## Tokens

Raw values are CSS custom properties per theme (`:root` light, `:root[data-theme="dark"]`, and the
dark values again under `prefers-color-scheme: dark` unless `data-theme="light"`). A Tailwind v4
`@theme inline` block maps them to utilities.

- Surfaces: `surface-0` (app background), `surface-1` (bars, sidebar, cards), `surface-2` (controls,
  hover), `surface-3` (tracks, strong hover). Borders: `border`, `border-strong`.
- Text: `fg` (primary), `fg-muted` (secondary), `fg-subtle` (muted, labels, units).
- Accent: `accent`, `accent-hover`, `accent-fg`, `accent-soft` (selected backgrounds).
- Status: `success`, `warn`, `danger`, each with a `-soft` background.
- Radius `rounded-sm/md/lg` = 6/8/12 px; spacing is Tailwind's 4 px scale; `shadow-pop` for overlays.
- Fonts: `font-sans` (Inter, system-ui, ...), `font-mono` (JetBrains Mono, ui-monospace, ...).
- Breakpoint `shell` = 900 px (sidebar becomes a drawer below it: `max-shell:`).
- Canvas colours live in `web/src/spectrum/palette.ts` and mirror the dark tokens.

## Layout

1. Top bar: wordmark, connection pill with device id/tuner/rate, the digit-tunable frequency
   readout of the device centre (read-only while the tuning lock is held), the padlock (tuning lock,
   with a "Locked by ..." badge), audio on/off, settings menu (theme).
2. Sidebar (320 px, collapsible sections, all open by default): Source, Profiles, VFOs (compact list
   with select/delete and the paused hint), Display, Audio device. Below 900 px a slide-over drawer
   opened from the menu button (Esc or backdrop closes it).
3. Main: a column of the spectrum card and the receiver dock (see "Receiver dock"). The spectrum card
   holds spectrum above waterfall filling the remaining height, draggable splitter (keyboard:
   arrows, Home resets; stored in `localStorage`), VFO overlays with passband shading and label
   chips (paused VFOs dashed, faded, "(paused)"), the parts of the span outside the usable window
   (85 % of the sample rate) shaded with dashed edge lines, frequency axis.
   - Waterfall history: every row keeps the absolute span it was rendered with (`spectrum/history.ts`);
     the shader maps each row onto the current view, so zoom, pan and centre retunes keep the history
     (areas a row never covered show the background). Rows are resampled to a fixed 4096-texel width
     and carry their own dB mapping; history is only lost with the WebGL context or the device.
   - Click preview: hovering draws a cursor line at the frequency a click would tune to (snapped to the
     step of the active VFO, whole Hz for a centre tune), a ghost passband of the active VFO and a
     chip "145.500 000 MHz  → VFO 1" (or "→ centre", "→ new VFO" with Alt/Ctrl, "(outside window)").
     A click glides the marker (150 ms ease-out) and pulses a fading line at the target, which keeps
     its frequency when the click retunes the centre. `prefers-reduced-motion` turns glide and pulse
     off. The overlay is a 2D canvas, so the preview also works without WebGL2.
   - Usable window: outside parts are shaded (50 %), edges dashed, labelled "outside tuning window"
     (or "outside" when narrow) at the top edge, with a tooltip explaining that VFOs there are paused
     or make the daemon retune; the exact edge frequencies are flagged on the frequency axis.
4. Status bar: buffer, drift, latency, underruns, dropped (audio samples replaced by silence),
   fps. The device-side `dropped_blocks` counter is not exposed to the browser yet.

Responsive down to 375 px: the top bar wraps (readout on its own row), the audio button shrinks to
an icon, the page never scrolls horizontally.

## Components (`web/src/ui/`)

Button (primary / secondary / ghost / danger, sm / md / lg), Chip (selectable pill), IconButton (label + tooltip, pressed),
Tooltip (CSS only, hover and focus), Select (styled native), Slider (label, value, overlay slot for
live markers), Toggle (switch), SegmentedControl (radio group, arrow keys), NumberField (typed
value, optional parse/format, arrow stepping), Section (collapsible) and Panel, Badge, Meter (bar,
marker, tick scale; can be driven imperatively), Field, EmptyState, Kbd, RowButton (selectable list
row), FrequencyReadout (digit-tunable), TextField and PasswordField (label, hint, error; show/hide
toggle), Alert (inline danger / warn / info message), Dialog (modal: portal, Esc and backdrop close,
focus trap and restore), ConfirmDialog (Dialog with cancel/confirm; information-only without
`onConfirm`), Toaster + `toasts` store (non-blocking notices: tones, replace-by-key, auto-dismiss,
polite live region, danger as alert; usable from plain TS), Logo (mark + wordmark), icons (inline SVG only), `theme.ts`.

## Tuning lock, free tuning, profiles

- The state socket (`state/`, `docs/protocol-ws.md`) is the only source of confirmed device state
  and VFOs; the controllers poll nothing (one REST read at start is the fallback). Control replies
  that carry a state document (lock, VFO create/update, profile load) are applied at once.
- Padlock (`shell/TuningLock`): pressed = user lock held. Locked: the centre readout, centre
  field and sample rate are disabled with the reason ("Locked by you. Unlock to change ..."),
  spectrum clicks and typed centre changes are refused client-side; a `409 locked` reply or a
  refused change raises one toast naming the holders. VFOs stay editable.
- Free tuning: a VFO moved outside the window makes the daemon retune (the view follows the new
  centre). With the lock held the VFO is `paused` and a toast says "Tuning locked — VFO paused
  outside the window"; a failed retune is a danger toast. Paused VFOs get a pause badge and hint in
  the list and a dashed, faded marker on the spectrum.
- Profiles (sidebar): list with name, centre, rate, VFO count and age; Load, Rename, Delete (with
  confirmation), "Save current as..." (name dialog; a name clash asks before overwriting). Loading
  a profile that would change centre or rate while locked opens a dialog with the reason and, if
  only the user lock is held, "Unlock and load".

## Auth screens (`web/src/auth/`)

Setup, login and the "cannot reach server" state share `AuthLayout`: a centred card (max 448 px,
scrolls on short screens) under the logo, in both themes, usable at 375 px. Forms use
`autocomplete` hints (`username`, `current-password`, `new-password`) so password managers work,
focus the first field and submit on Enter. Errors are inline `Alert`s or field errors; rate limiting
disables the submit button with a seconds countdown. A 401 from any request or an auth-rejected
WebSocket returns the user to the login page; sockets do not reconnect while logged out.

## Rules

- No raw `<button>` or `<select>` outside `web/src/ui/` (ESLint `no-restricted-syntax`).
- Colours only through tokens (`bg-surface-1`, `text-fg-muted`, ...); no hard-coded hex in
  components. Canvas renderers use `spectrum/palette.ts`.
- Every control has an accessible name (visible label, `aria-label` or IconButton `label`) and a
  visible keyboard focus state.
- Hot paths (spectrum, waterfall, meters) update the DOM/canvas imperatively; React state changes
  only on coarse events.
- Inline SVG icons only; no icon or UI-kit dependencies.

## Frequency readout

`FrequencyReadout` shows ten digits (0000.000.000 MHz.kHz.Hz, leading zeros dimmed). Each digit is a
`spinbutton`: wheel and ArrowUp/ArrowDown step it with carry and clamping, ArrowLeft/Right/Home/End
move focus, digit keys overwrite, Enter/F2/double-click/pencil switch to typed entry with unit
parsing. Pure arithmetic is in `ui/digits.ts`.

## Receiver dock

`vfo/ReceiverDock.tsx` sits under the spectrum card (`shrink-0`, the card takes the remaining height)
and holds every operating control of the active VFO, so nothing needs a sidebar visit:

- VFO chips (selected = accent, paused = dimmed with a pause icon) plus an add button.
- Digit-tunable readout of the active VFO flanked by down/up nudge buttons, and the tuning step select.
- Mode `SegmentedControl` (all modes; a change resets the bandwidth to the mode default) and bandwidth.
- Squelch slider with the live channel-power tick and a compact signal meter (both update their DOM
  outside React), volume, mute, audio on/off and delete of the active VFO.

The active VFO is always defined: `useReceiver` selects the first VFO whenever VFOs exist and none is
selected (there is no deselect). The readout in the top bar therefore only tunes the device centre.
A plain click on the spectrum retunes the active VFO (SDR#-style; drag, or the top bar readout and
the Source section, move the centre); with no VFO it tunes the centre. The tuning lock behaves as
before: the centre is read-only, VFOs stay tunable and are paused outside the window.

Layout uses a container query on the dock, not the viewport: the groups wrap inside two bands (VFO and
tuning, then demodulator and audio). From 88 rem (about 1400 px) of dock width the bands sit side by
side in one row; narrower, they stack in two rows; groups get a subtle vertical divider from 56 rem and
simply wrap below that. On phones (below the 900 px shell breakpoint, where the sidebar becomes a
drawer) the dock stays visible, wraps further and scrolls vertically (max 45 % of the viewport)
rather than the page scrolling horizontally. With no VFOs the dock shrinks to an "Add VFO" prompt.
