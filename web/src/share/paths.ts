// URLs of the public share pages (ADR-0029). The token is a secret: it only ever appears in a
// path, never in a query string, and the pages send no referrer to other origins.

/** Page path of a share token: the full public receiver. */
export function receiverPath(token: string): string {
  return `/r/${encodeURIComponent(token)}`;
}

/** Page path of a share token: the compact player meant for an iframe. */
export function embedPath(token: string): string {
  return `/embed/${encodeURIComponent(token)}`;
}

/** The token of `/r/<token>` or `/embed/<token>`; `null` for any other path. */
export function sharePageOf(path: string): { kind: "receiver" | "embed"; token: string } | null {
  const m = /^\/(r|embed)\/([^/?#]+)\/?$/.exec(path);
  if (!m?.[1] || !m[2]) return null;
  try {
    return { kind: m[1] === "r" ? "receiver" : "embed", token: decodeURIComponent(m[2]) };
  } catch {
    return null;
  }
}

/** Absolute link for visitors. */
export function receiverUrl(token: string, origin: string = window.location.origin): string {
  return origin + receiverPath(token);
}

/** HTML snippet that embeds the player. */
export function embedCode(token: string, origin: string = window.location.origin): string {
  const src = origin + embedPath(token);
  return (
    `<iframe src="${src}" width="420" height="260" style="border:0" ` +
    `allow="autoplay" loading="lazy" title="Live radio receiver"></iframe>`
  );
}

/** API paths of one share token. */
export function shareApi(token: string) {
  const base = `/api/v1/share/${encodeURIComponent(token)}`;
  return {
    info: base,
    state: `${base}/state`,
    spectrum: (loc: Pick<Location, "protocol" | "host"> = location) =>
      ws(loc, `${base}/ws/spectrum`),
    audio: (loc: Pick<Location, "protocol" | "host"> = location) => ws(loc, `${base}/ws/audio`),
    events: (loc: Pick<Location, "protocol" | "host"> = location) => ws(loc, `${base}/ws/events`),
  };
}

function ws(loc: Pick<Location, "protocol" | "host">, path: string): string {
  return `${loc.protocol === "https:" ? "wss:" : "ws:"}//${loc.host}${path}`;
}
