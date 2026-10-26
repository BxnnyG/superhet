// Tiny client-side router: the app has three kinds of URL (the receiver at `/`, the admin
// console under `/admin`, invitation links under `/invite/<token>`), so the History API plus a
// store hook is all it needs. The API serves index.html for every unknown non-API path.

import { useSyncExternalStore } from "react";

const CHANGE_EVENT = "superhet:navigate";

export function currentPath(): string {
  return window.location.pathname;
}

/** Go to another path without a page load (push by default). */
export function navigate(path: string, opts: { replace?: boolean } = {}): void {
  if (opts.replace) window.history.replaceState(null, "", path);
  else window.history.pushState(null, "", path);
  window.dispatchEvent(new Event(CHANGE_EVENT));
}

function subscribe(listener: () => void): () => void {
  window.addEventListener("popstate", listener);
  window.addEventListener(CHANGE_EVENT, listener);
  return () => {
    window.removeEventListener("popstate", listener);
    window.removeEventListener(CHANGE_EVENT, listener);
  };
}

/** The current location path; re-renders on navigation and the back/forward buttons. */
export function usePath(): string {
  return useSyncExternalStore(subscribe, currentPath, () => "/");
}

/** Token of an invitation link `/invite/<token>`, or null for any other path. */
export function inviteTokenOf(path: string): string | null {
  const m = /^\/invite\/([^/?#]+)\/?$/.exec(path);
  if (!m?.[1]) return null;
  try {
    return decodeURIComponent(m[1]);
  } catch {
    return null;
  }
}

/** Section of the admin console (`/admin/<section>`); null when the path is not the console. */
export function adminSectionOf(path: string): string | null {
  const m = /^\/admin(?:\/([a-z-]+))?\/?$/.exec(path);
  return m ? (m[1] ?? "") : null;
}
