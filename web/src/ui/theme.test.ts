import { afterEach, expect, it, vi } from "vitest";
import { THEME_STORAGE_KEY, applyTheme, loadTheme, nextTheme, saveTheme } from "./theme";

afterEach(() => {
  vi.restoreAllMocks();
  window.localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
});

it("defaults to system and round-trips a stored choice", () => {
  expect(loadTheme()).toBe("system");
  saveTheme("dark");
  expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
  expect(loadTheme()).toBe("dark");
  saveTheme("system");
  expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
});

it("ignores garbage and survives throwing storage", () => {
  window.localStorage.setItem(THEME_STORAGE_KEY, "neon");
  expect(loadTheme()).toBe("system");
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
    throw new Error("denied");
  });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
    throw new Error("denied");
  });
  expect(loadTheme()).toBe("system");
  expect(() => saveTheme("light")).not.toThrow();
});

it("sets and clears data-theme", () => {
  applyTheme("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  applyTheme("system");
  expect(document.documentElement.dataset.theme).toBeUndefined();
});

it("cycles through the preferences", () => {
  expect(nextTheme("system")).toBe("light");
  expect(nextTheme("light")).toBe("dark");
  expect(nextTheme("dark")).toBe("system");
});
