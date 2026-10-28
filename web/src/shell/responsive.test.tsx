import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { App } from "../App";
import { INSPECTOR_STORAGE_KEY } from "./inspector";
import { PHONE_QUERY, SHELL_QUERY } from "./useMediaQuery";

/** Pretend the viewport matches only the given queries (jsdom has no matchMedia). */
function viewport(matching: string[]) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: matching.includes(query),
    media: query,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
  }));
}

afterEach(() => {
  vi.unstubAllGlobals();
  window.localStorage.removeItem(INSPECTOR_STORAGE_KEY);
});

it("the page shell never scrolls: fixed height, hidden overflow", () => {
  const { container } = render(<App />);
  const shell = container.firstElementChild;
  expect(shell).toHaveClass("h-full", "overflow-hidden", "flex-col");
});

it("tablet width: the inspector is a closed sheet that ] and its close button toggle", () => {
  viewport([PHONE_QUERY]);
  render(<App />);
  expect(screen.queryByRole("tablist", { name: "Inspector" })).not.toBeNull();
  const aside = screen.getByRole("complementary", { name: "Inspector", hidden: true });
  expect(aside).toHaveClass("max-shell:invisible");
  fireEvent.keyDown(window, { key: "]" });
  expect(aside).not.toHaveClass("max-shell:invisible");
  fireEvent.keyDown(window, { key: "Escape" });
  expect(aside).toHaveClass("max-shell:invisible");
  fireEvent.keyDown(window, { key: "]" });
  fireEvent.click(screen.getByRole("button", { name: "Close inspector" }));
  expect(aside).toHaveClass("max-shell:invisible");
  // The sheet does not touch the persisted desktop preference.
  expect(window.localStorage.getItem(INSPECTOR_STORAGE_KEY)).toBeNull();
});

it("phone width: a bottom tab bar opens the inspector sheet on a tab and moves mode controls in", () => {
  viewport([]);
  render(<App />);
  const bar = screen.getByRole("navigation", { name: "Panels" });
  const buttons = bar.querySelectorAll("button");
  expect([...buttons].map((b) => b.textContent)).toEqual([
    "Receiver",
    "Bookmarks",
    "Channels",
    "Decoders",
    "DAB",
    "ADS-B",
    "Display",
    "Source",
  ]);
  const display = screen.getByRole("button", { name: "Display" });
  expect(display).toHaveAttribute("aria-pressed", "false");
  fireEvent.click(display);
  expect(display).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByRole("switch", { name: "Automatic levels" })).toBeInTheDocument();
  fireEvent.click(display);
  expect(display).toHaveAttribute("aria-pressed", "false");
  // The strip has no mode chips on phones; they live in the Receiver sheet.
  expect(screen.queryByRole("radiogroup", { name: "Mode" })).toBeNull();
});

it("wide screens have no tab bar", () => {
  viewport([SHELL_QUERY, PHONE_QUERY]);
  render(<App />);
  expect(screen.queryByRole("navigation", { name: "Panels" })).toBeNull();
});
