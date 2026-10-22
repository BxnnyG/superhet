import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { getWheelMode, resetWheelModeCache, setWheelMode } from "../spectrum/wheelMode";
import { SettingsMenu } from "./SettingsMenu";

afterEach(() => {
  window.localStorage.clear();
  resetWheelModeCache();
});

it("switches the mouse wheel between zoom and tune and remembers it", () => {
  render(<SettingsMenu theme="dark" onTheme={() => undefined} onShortcuts={() => undefined} />);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  expect(getWheelMode()).toBe("zoom");
  fireEvent.click(screen.getByRole("radio", { name: "Tune" }));
  expect(getWheelMode()).toBe("tune");
  expect(window.localStorage.getItem("superhet.wheelMode")).toBe("tune");
  resetWheelModeCache();
  expect(getWheelMode()).toBe("tune");
  setWheelMode("zoom");
  expect(window.localStorage.getItem("superhet.wheelMode")).toBe("zoom");
});

it("opens the keyboard shortcuts from the menu", () => {
  const onShortcuts = vi.fn();
  render(<SettingsMenu theme="dark" onTheme={() => undefined} onShortcuts={onShortcuts} />);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("button", { name: /Keyboard shortcuts/ }));
  expect(onShortcuts).toHaveBeenCalled();
});
