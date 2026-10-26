import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { newInput, type Bookmark } from "../bookmarks/model";
import { Palette } from "./Palette";
import type { PaletteDeps } from "./items";

const bookmark: Bookmark = {
  ...newInput({ name: "Munich Tower", freqHz: 118_100_000, mode: "am", tags: ["airband"] }),
  id: "a",
  createdMs: 0,
  updatedMs: 0,
};

function setup() {
  const calls = {
    tune: vi.fn(),
    remember: vi.fn(),
    act: vi.fn(),
    selectVfo: vi.fn(),
    showTab: vi.fn(),
    toggleAudio: vi.fn(),
    exportLog: vi.fn(),
    openSettings: vi.fn(),
    onClose: vi.fn(),
  };
  const deps: PaletteDeps = {
    vfos: [],
    selectedId: null,
    bookmarks: [bookmark],
    recent: [],
    audioEnabled: false,
    ...calls,
  };
  render(
    <>
      <button type="button">before</button>
      <Palette deps={deps} onClose={calls.onClose} />
    </>,
  );
  return { calls, input: screen.getByRole("combobox") };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const flush = () => act(() => vi.runAllTimers());

describe("Palette", () => {
  it("is a modal dialog with a combobox, a listbox and options", () => {
    const { input } = setup();
    expect(screen.getByRole("dialog", { name: "Command palette" })).toHaveAttribute(
      "aria-modal",
      "true",
    );
    expect(input).toHaveFocus();
    expect(screen.getByRole("listbox")).toBeInTheDocument();
    expect(screen.getAllByRole("option").length).toBeGreaterThan(3);
    const active = input.getAttribute("aria-activedescendant");
    expect(active).not.toBeNull();
    expect(document.getElementById(active ?? "")).toHaveAttribute("aria-selected", "true");
  });

  it("moves the active option with the arrow keys and wraps", () => {
    const { input } = setup();
    const options = () => screen.getAllByRole("option");
    expect(options()[0]).toHaveAttribute("aria-selected", "true");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    expect(options()[1]).toHaveAttribute("aria-selected", "true");
    expect(input.getAttribute("aria-activedescendant")).toBe(options()[1]?.id);
    fireEvent.keyDown(input, { key: "ArrowUp" });
    fireEvent.keyDown(input, { key: "ArrowUp" });
    expect(options().at(-1)).toHaveAttribute("aria-selected", "true");
  });

  it("tunes a typed frequency with Enter and in a new VFO with Alt+Enter", () => {
    const { calls, input } = setup();
    fireEvent.change(input, { target: { value: "446.00625 nfm ctcss 88.5" } });
    expect(screen.getAllByRole("option")[0]).toHaveTextContent("Tune to 446.006 250 MHz");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(calls.onClose).toHaveBeenCalledTimes(1);
    flush();
    expect(calls.tune).toHaveBeenLastCalledWith(
      { freqHz: 446_006_250, mode: "nfm", toneSquelchHz: 88.5 },
      false,
    );
    fireEvent.keyDown(input, { key: "Enter", altKey: true });
    flush();
    expect(calls.tune).toHaveBeenLastCalledWith(expect.anything(), true);
  });

  it("runs a bookmark by click", () => {
    const { calls, input } = setup();
    fireEvent.change(input, { target: { value: "tower" } });
    fireEvent.click(screen.getByRole("option", { name: /Munich Tower/ }));
    flush();
    expect(calls.tune).toHaveBeenCalledWith(
      expect.objectContaining({ freqHz: 118_100_000, mode: "am" }),
      false,
    );
  });

  it("shows the shortcut of an action", () => {
    const { input } = setup();
    fireEvent.change(input, { target: { value: "bookmark here" } });
    const option = screen.getByRole("option", { name: /Add a bookmark here/ });
    expect(option).toHaveTextContent("B");
  });

  it("keeps focus in the field on Tab and closes on Escape", () => {
    const { calls, input } = setup();
    fireEvent.keyDown(input, { key: "Tab" });
    expect(input).toHaveFocus();
    fireEvent.keyDown(input, { key: "Escape" });
    expect(calls.onClose).toHaveBeenCalled();
  });

  it("says so when nothing matches", () => {
    const { input } = setup();
    fireEvent.change(input, { target: { value: "qqqqzzzz" } });
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText(/Nothing matches/)).toBeInTheDocument();
  });
});
