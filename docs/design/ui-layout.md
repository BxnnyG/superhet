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
- **Spectrum and waterfall** fill the remaining height (`flex-1 min-h-0`). Overlays: band ranges as a
  thin ribbon above the scale, point bookmarks as clickable pins, VFO passbands.
- **Right inspector** (w-80, collapsible, `]` toggles it, state persisted in `localStorage`) with the
  tabs Receiver, Bookmarks, Decoders, Display, Source. Every tab body scrolls on its own.
  - Receiver: the selected VFO in detail (stereo and de-emphasis, output volume, RDS or CTCSS/DTMF,
    remove).
  - Bookmarks: the bookmark panel.
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
  frequency; the spectrum; a bottom tab bar (Receiver, Bookmarks, Display, Source) opens bottom
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

## Later

- Mobile sheets polish and the scanner UI (the scanner backend needs an ADR first).
