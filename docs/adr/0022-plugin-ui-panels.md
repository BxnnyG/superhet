# ADR-0022: Plugin web panels in a sandboxed frame

**Status:** accepted (2026-11-07)

## Context

ARCHITECTURE 6.6 and SCOPE sketched UI plugins as ES modules the host loads into the page
("single user, no hard sandbox, plugins are trusted code"). That was written before plugins became
processes with a contract, before roles (ADR-0011) and before the product grew listeners and
guests who may see decoder output. A module in the page runs with the viewer's session: it could
call every API the viewer may call (stop decoders, change the device, read the audit log as an
admin), read what the app keeps in the page, and a plugin update becomes a code update of the web
app. ROADMAP P5 asks for a documented UI plugin API and first panels that load from their own
packages without changes to the core frontend. A decoder with its own view (a map, a call log, a
waterfall of one channel) is the use case.

## Decision

1. **A panel is a static web page in the plugin package**, declared in the manifest:
   `[ui] entry = "ui/index.html"`, `title`, `placement = "inspector"`, optional `reads` and `tune`.
   The directory of `entry` is the *UI root*; only files below it can be served, never the plugin's
   binaries, schemas or configuration (the entry must live in a subdirectory). `[ui]` is required
   for `type = "ui"` and optional for every other type, so a decoder can ship its own view; a
   `ui` plugin has no instances and names the plugins it reads. The old `ui.widget` (ES module in
   the host page) is removed; no plugin used it.
2. **Served by the API from the plugin host.** The API does not see the plugin directory. A new
   plugin host command `asset` (`sh.v1.plugin.ctl.asset`) returns one file; the API serves it at
   `GET /api/v1/plugin-ui/{plugin_id}/{path}`. Paths are validated by one rule
   (`superhet_plugin_protocol::ui`): segments of `[A-Za-z0-9._-]`, none empty or starting with a
   dot, at most 8 deep, nothing else is normalised, it is refused. The host additionally resolves
   the file and requires it to equal `<canonical UI root>/<path>` (links anywhere below the root are
   refused), a regular file, at most 640 KiB (the bus payload limit), of a served type (html, js,
   css, json, txt, svg, png, jpg, gif, webp, woff2; no wasm, no maps). The API keeps a copy of the
   validators (it does not link the plugin protocol crate) and a test pins them to the original.
3. **The route is public and every response is sandboxed.** A frame without `allow-same-origin`
   has an opaque origin, so its sub-resource requests cannot carry the `SameSite=Strict` session
   cookie; requiring a session would break the panel's scripts and styles, and a design where the
   frame is allowed to send the cookie is exactly what must not exist. The files are plugin code an
   admin installed (like the static web UI they need no secrecy; plugins must not keep secrets in
   their UI directory). Every response carries `Content-Security-Policy: sandbox allow-scripts;
   default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; font-src 'self';
   connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors
   'self'`, plus `nosniff`, `Referrer-Policy: no-referrer`, a restrictive `Permissions-Policy` and
   `Access-Control-Allow-Origin: *` (module scripts and fonts of an opaque origin are fetched with
   CORS; the files are public and credential-free). The `sandbox` directive in the header means
   the page is opaque even when someone opens the URL in a tab, where it would otherwise be
   same-origin with the app.
4. **The web app embeds it with `sandbox="allow-scripts"`**, `allow=""`, `referrerpolicy=
   "no-referrer"`. No `allow-same-origin`, `allow-top-navigation`, `allow-popups`, `allow-forms`,
   `allow-modals`, `allow-downloads`. The page is an inspector tab ("Plugins"; a picker when
   several plugins ship a panel), visible to everybody who may see decoder output; the list comes
   from `GET /api/v1/plugin-panels` (`GuestDecoder`, a whitelist of manifest fields).
   `placement = "dock"` is not implemented; the manifest rejects unknown placements.
5. **Communication is a versioned `postMessage` bridge, host-driven** (`docs/plugins-ui-panels.md`).
   The web app reads with its own session and sends the panel a filtered copy: instances of the
   plugins in `reads` (id, state, binding; never configuration, process ids, error text or
   counters), the newest 50 events of those plugins, theme tokens. The panel may send exactly two
   messages: `hello` (start) and `tune` (a frequency). Messages are accepted only from the frame's
   own window with origin `"null"`, must be plain objects with exactly the fields of their type
   and `superhet: 1`; everything else is dropped silently. `tune` is honoured only if the manifest
   sets `tune = true` **and** the viewer is an operator or admin (the UI check mirrors what the
   API would enforce for that user anyway), at most once per second, and applies to the selected
   receiver through the normal VFO path. The panel has no other way to cause an effect.
6. **A frame that navigates away is closed.** The sandbox cannot forbid a page from navigating its
   own frame; a second `load` event removes the frame so the bridge never talks to another page.

## Consequences

- A plugin cannot act as the viewer: no cookies or storage (opaque origin), no `fetch`/XHR/
  WebSocket/beacon (`connect-src 'none'`), no access to the parent, no new windows, no forms. A
  compromised or hostile panel can still *show* misleading content inside its tab, and it can
  leak the data it is given (decoder events of the plugins it reads, which the viewer may see
  anyway) by navigating the frame or its resources to a foreign URL: browsers have no
  `navigate-to`. The frame is then closed, but the request has gone out. Installing a plugin is an
  admin decision (manifest and licence notice are shown first, ADR-0014); the sandbox protects the
  session, the API and the app, not the confidentiality of what the panel is shown.
- Panels are plain HTML/JS/CSS with no framework requirement and no build step in the core. They
  cannot import from the app and cannot use the app's components; theme tokens
  (`--sh-surface-1`, `--sh-fg`, ...) are the only styling bridge.
- Because the API is the edge, a panel is served only if the plugin host answers; a plugin
  update replaces the files with the next restart of the host (the web app adds `?v=<version>` to
  bust the cache; responses are `no-cache` anyway).
- The old idea of UI plugins registering command-palette entries or widgets in the page is
  dropped; if it is needed it becomes new bridge messages with their own gating, never page access.
- Dock placement and a ready-made client library for panels are later additions; the bridge
  version in every message lets them arrive without breaking existing panels.
- ROADMAP P5 criterion 2 (UI plugins load from their own packages) has its mechanism now; the
  ADS-B map (ADR-0017) and the future TETRA cell map are not ported and stay core components.
