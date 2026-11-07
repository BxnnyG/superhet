# Plugin web panels

A plugin can ship a small web page that the web app shows in the **Plugins** inspector tab: a custom
view of a decoder's events, a status page, a map of its own. It runs in a sandboxed frame and talks
to the app through a message bridge. Decision and threat model: ADR-0022. What users see of plugins
in general: `docs/plugins-ui.md`; the manifest: `docs/plugin-contract.md`. The reference is
`plugins/example-echo/ui/`.

## Manifest

```toml
[ui]
entry = "ui/index.html"          # required; an .html file inside a subdirectory of the package
title = "Echo view"              # required; 1-32 plain characters (the tab picker's label)
placement = "inspector"          # optional; the only value so far
reads = ["org.superhet.decoder.aprs"]   # optional; plugin ids the panel may read (default: this plugin)
tune = false                     # optional; the panel may ask for a tune request (operators/admins only)
```

- `[ui]` is required for `type = "ui"` (no process, `reads` must name at least one plugin) and
  optional for decoders, codecs and lookups.
- The directory of `entry` (`ui/`) is the **UI root**. Only files below it are served. Put nothing
  secret in it: the files are public (see below).
- Served file types: `html js mjs css json txt svg png jpg jpeg gif webp woff2`, each at most
  640 KiB, paths of `[A-Za-z0-9._-]` segments (at most 8 deep, none starting with a dot), no
  symbolic links. Other files in the directory are simply not served.
- Install checks the manifest like any other; `check_static` (the contract suite) also checks that
  `entry` exists.

## Serving and what the page is allowed to do

`GET /api/v1/plugin-ui/{plugin_id}/{path}` (path below the UI root; the page is `index.html`, so
relative URLs such as `panel.js` work) answers from the plugin host's copy of the active version.
**No session is needed or sent**: a sandboxed frame has no cookie, which is the point. Every
response carries

```
Content-Security-Policy: sandbox allow-scripts; default-src 'none'; script-src 'self';
  style-src 'self'; img-src 'self' data:; font-src 'self'; media-src 'none'; connect-src 'none';
  frame-src 'none'; object-src 'none'; worker-src 'none'; manifest-src 'none';
  form-action 'none'; base-uri 'none'; frame-ancestors 'self'
X-Content-Type-Options: nosniff     Referrer-Policy: no-referrer
Access-Control-Allow-Origin: *      Cross-Origin-Resource-Policy: cross-origin
```

and the web app embeds it as `<iframe sandbox="allow-scripts" allow="" referrerpolicy="no-referrer">`.
For the panel's author that means:

- Scripts and styles must be **files** next to the page (`<script src>`, `<link rel=stylesheet>`);
  inline `<script>`, `<style>`, `style=""` attributes, `eval` and event-handler attributes are
  blocked. Setting styles from script through the CSSOM (`el.style.x = ...`) works.
- No network: `fetch`, XHR, WebSocket, `sendBeacon`, external images/fonts/scripts are all blocked.
  The page cannot call the Superhet API, with or without credentials.
- Opaque origin: no cookies, `localStorage`, IndexedDB or access to the parent beyond `postMessage`.
- No popups, forms, downloads, modals, top navigation, camera, microphone, geolocation.
- Do not navigate the frame: the app closes a frame that loads a second page.
- Use `textContent`, not `innerHTML`, for anything from the air (callsigns, messages).

## The bridge (version 1)

Messages are structured-cloneable plain objects with `superhet: 1` and a `type`. The panel posts to
`window.parent` with target origin `"*"` (its own origin is opaque) and should only believe messages
whose `event.source === window.parent`.

**Panel to app** (the complete vocabulary; anything else, including extra fields, is dropped
without an answer):

| Message | Meaning |
|---|---|
| `{superhet:1, type:"hello"}` | Send first, and again after a reload; the app answers with `init`, `instances` and `events`. Nothing is pushed before it. |
| `{superhet:1, type:"tune", freq_hz:100000000}` | Tune the selected receiver (a VFO; one is created if there is none). `freq_hz` an integer, 1 to 6 000 000 000. Answered with `tune_result`. |

**App to panel**:

| Message | Payload |
|---|---|
| `init` | `plugin_id`, `can_tune` (bool: manifest `tune` **and** the viewer is an operator or admin), `theme` |
| `theme` | `theme`; resent when the user switches light/dark |
| `instances` | `instances`: `[{id, plugin_id, device, vfo, freq_hz, state, updated_ms}]` of the plugins in `reads` that are not stopped; resent when they change. No configuration, process id, error text or counters. |
| `events` | `events`: the newest 50 decoder events of the plugins in `reads`, newest first: `{id, ts_ms, source, kind, value, device, vfo, freq_hz, meta}`; resent when new ones arrive. Roles: the panel sees what its viewer sees (guests only when the guest policy shows decoder output). |
| `tune_result` | `ok`, and `error` when `false` (`not allowed`, `too many requests`) |

`theme` is a map of colours (`surface-0..3`, `border`, `border-strong`, `fg`, `fg-muted`,
`fg-subtle`, `accent`, `accent-hover`, `accent-fg`, `accent-soft`, `success`, `warn`, `danger`) plus
`scheme` (`light` or `dark`). The example sets them as CSS variables `--sh-<name>`.

Tune requests are limited to one per second. Everything else about the app (VFOs, device settings,
other plugins, users) is out of reach. New message types or fields come with a new `superhet`
version; a panel written for 1 keeps working.

## Who sees a panel

`GET /api/v1/plugin-panels` lists the panels of the installed plugins (`plugin_id`, `name`,
`version`, `title`, `placement`, `entry`, `reads`, `tune`; never the runtime or install sections).
It is open to everybody who may see decoder output (listeners, guests when the guest policy shows
decoder events). The tab appears when at least one panel exists; the list refreshes every 30 s.

## Writing a panel

Copy `plugins/example-echo/ui/` (`index.html`, `panel.js`, `panel.css`) and the `[ui]` section.
The example is about 80 lines of dependency-free script; a ready-made client library is not part of
the first version. Test it by installing the plugin from its directory (`POST /api/v1/plugins` or
the admin console) and opening the Plugins tab.
