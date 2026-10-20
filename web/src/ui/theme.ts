// Theme preference: "system" follows prefers-color-scheme, "light"/"dark" force one.
// Persisted in localStorage (every storage access is guarded; private mode may throw).

import { useCallback, useEffect, useState } from "react";

export type ThemePreference = "system" | "light" | "dark";

export const THEME_STORAGE_KEY = "superhet.theme";
export const THEME_PREFERENCES: readonly ThemePreference[] = ["system", "light", "dark"];

export function isThemePreference(v: unknown): v is ThemePreference {
  return v === "system" || v === "light" || v === "dark";
}

export function loadTheme(): ThemePreference {
  try {
    const raw = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isThemePreference(raw) ? raw : "system";
  } catch {
    return "system";
  }
}

export function saveTheme(pref: ThemePreference): void {
  try {
    if (pref === "system") window.localStorage.removeItem(THEME_STORAGE_KEY);
    else window.localStorage.setItem(THEME_STORAGE_KEY, pref);
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

/** Set or clear `data-theme` on <html>; without it the media query decides. */
export function applyTheme(pref: ThemePreference, root: HTMLElement = document.documentElement) {
  if (pref === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", pref);
}

export function nextTheme(pref: ThemePreference): ThemePreference {
  return (
    THEME_PREFERENCES[(THEME_PREFERENCES.indexOf(pref) + 1) % THEME_PREFERENCES.length] ?? "system"
  );
}

export function useTheme(): [ThemePreference, (pref: ThemePreference) => void] {
  const [pref, setPref] = useState<ThemePreference>(loadTheme);
  useEffect(() => {
    applyTheme(pref);
  }, [pref]);
  const set = useCallback((next: ThemePreference) => {
    saveTheme(next);
    setPref(next);
  }, []);
  return [pref, set];
}
