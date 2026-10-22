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
   readout (VFO if one is selected, else device centre; a badge names the target; read-only for the
   centre while the tuning lock is held), the padlock (tuning lock, with a "Locked by ..." badge),
   volume of the selected VFO, audio on/off, settings menu (theme).
2. Sidebar (320 px, collapsible sections): Source, Profiles, Radio, Display, Audio. Below 900 px a slide-over
   drawer opened from the menu button (Esc or backdrop closes it).
3. Main: spectrum above waterfall filling the remaining height, draggable splitter (keyboard:
   arrows, Home resets; stored in `localStorage`), VFO overlays with passband shading and label
   chips (paused VFOs dashed, faded, "(paused)"), the parts of the span outside the usable window
   (85 % of the sample rate) shaded with dashed edge lines, frequency axis.
4. Status bar: buffer, drift, latency, underruns, dropped (audio samples replaced by silence),
   fps. The device-side `dropped_blocks` counter is not exposed to the browser yet.

Responsive down to 375 px: the top bar wraps (readout on its own row), the audio button shrinks to
an icon, the page never scrolls horizontally.

## Components (`web/src/ui/`)

Button (primary / secondary / ghost / danger, sm / md / lg), IconButton (label + tooltip, pressed),
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
