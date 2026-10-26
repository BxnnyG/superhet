// Display helpers shared by the sessions dialog and the admin console.

const pad = (n: number) => String(n).padStart(2, "0");

/** Local date and time, `2026-10-26 21:04:05`. */
export function formatDateTime(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** `5 s ago`, `3 min ago`, `2 h ago`, `4 d ago`; future times read `in 3 min`. */
export function formatRelative(ms: number, now: number = Date.now()): string {
  const diff = Math.round((now - ms) / 1000);
  const abs = Math.abs(diff);
  let text: string;
  if (abs < 5) return "just now";
  if (abs < 60) text = `${abs} s`;
  else if (abs < 3600) text = `${Math.round(abs / 60)} min`;
  else if (abs < 86_400) text = `${Math.round(abs / 3600)} h`;
  else text = `${Math.round(abs / 86_400)} d`;
  return diff >= 0 ? `${text} ago` : `in ${text}`;
}

/** `1 h 05 min`, `12 min`, `45 s` for a duration in seconds. */
export function formatDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return m === 0 ? `${h} h` : `${h} h ${pad(m)} min`;
}

/** "Firefox on Linux" from a user agent string; the raw string if nothing is recognised. */
export function describeUserAgent(ua: string | null): string {
  if (!ua) return "Unknown browser";
  const browser =
    /Edg\//.test(ua) || /EdgA\//.test(ua)
      ? "Edge"
      : /OPR\/|Opera/.test(ua)
        ? "Opera"
        : /Firefox\//.test(ua) || /FxiOS\//.test(ua)
          ? "Firefox"
          : /Chrome\/|CriOS\//.test(ua)
            ? "Chrome"
            : /Safari\//.test(ua)
              ? "Safari"
              : /curl\//.test(ua)
                ? "curl"
                : null;
  const os = /Android/.test(ua)
    ? "Android"
    : /iPhone|iPad|iPod/.test(ua)
      ? "iOS"
      : /Windows/.test(ua)
        ? "Windows"
        : /Mac OS X|Macintosh/.test(ua)
          ? "macOS"
          : /CrOS/.test(ua)
            ? "ChromeOS"
            : /Linux|X11/.test(ua)
              ? "Linux"
              : null;
  if (browser && os) return `${browser} on ${os}`;
  return browser ?? os ?? (ua.length > 40 ? `${ua.slice(0, 40)}…` : ua);
}

/** MHz with the digits that matter: `100.100`, `433.9200`. */
export function formatMhz(hz: number): string {
  return (hz / 1e6).toFixed(hz % 1000 === 0 ? 3 : 6).replace(/(\.\d{3})0+$/, "$1");
}
