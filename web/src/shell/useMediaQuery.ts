import { useSyncExternalStore } from "react";

/** Layout tiers; keep in step with --breakpoint-shell / --breakpoint-phone in index.css. */
export const SHELL_QUERY = "(min-width: 900px)";
export const PHONE_QUERY = "(min-width: 600px)";

/** Live `matchMedia` result; `fallback` where the API is missing (tests, old browsers). */
export function useMediaQuery(query: string, fallback = true): boolean {
  return useSyncExternalStore(
    (notify) => {
      if (typeof window.matchMedia !== "function") return () => undefined;
      const mql = window.matchMedia(query);
      mql.addEventListener("change", notify);
      return () => mql.removeEventListener("change", notify);
    },
    () => (typeof window.matchMedia === "function" ? window.matchMedia(query).matches : fallback),
    () => fallback,
  );
}
