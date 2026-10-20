import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it } from "vitest";
import { App } from "./App";

it("renders the SDR shell: top bar, sidebar sections, spectrum and status bar", () => {
  render(<App />);
  expect(screen.getByText("Superhet")).toBeInTheDocument();
  expect(screen.getByRole("group", { name: "Centre frequency" })).toBeInTheDocument();
  expect(screen.getByRole("complementary", { name: "Controls" })).toBeInTheDocument();
  for (const name of ["Source", "Radio", "Display", "Audio"]) {
    expect(screen.getByRole("button", { name })).toBeInTheDocument();
  }
  expect(screen.getByText("No VFOs yet")).toBeInTheDocument();
  expect(screen.getByRole("contentinfo", { name: "Status" })).toBeInTheDocument();
});

it("opens and closes the drawer from the menu button", () => {
  render(<App />);
  const menu = screen.getByRole("button", { name: "Open controls" });
  expect(menu).toHaveAttribute("aria-expanded", "false");
  fireEvent.click(menu);
  expect(menu).toHaveAttribute("aria-expanded", "true");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(menu).toHaveAttribute("aria-expanded", "false");
});

it("switches the theme from the settings menu", () => {
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Settings" }));
  fireEvent.click(screen.getByRole("radio", { name: "Dark" }));
  expect(document.documentElement.dataset.theme).toBe("dark");
  window.localStorage.clear();
  document.documentElement.removeAttribute("data-theme");
});
