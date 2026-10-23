import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Vfo } from "../vfo/api";
import { toasts } from "../ui";
import { shortcutFor, ignoresShortcuts, type KeyLike } from "./shortcuts";
import { runShortcut, useShortcuts, type ShortcutContext } from "./useShortcuts";

const key = (k: string, extra: Partial<KeyLike> = {}): KeyLike => ({
  key: k,
  shiftKey: false,
  ctrlKey: false,
  altKey: false,
  metaKey: false,
  ...extra,
});

function vfo(id: number, patch: Partial<Vfo> = {}): Vfo {
  return {
    id,
    freqHz: 100_000_000 + id * 1_000_000,
    mode: "wfm",
    bandwidthHz: 180_000,
    squelchDbfs: null,
    deemphasis: null,
    offsetHz: 0,
    status: "active",
    ...patch,
  };
}

function context(vfos: Vfo[], selectedId: number | null, patch: Partial<ShortcutContext> = {}) {
  const calls = {
    update: vi.fn(),
    select: vi.fn(),
    remove: vi.fn(),
    create: vi.fn(),
    setListening: vi.fn(),
    toggle: vi.fn(),
    zoom: vi.fn(),
    resetZoom: vi.fn(),
    createVfoAt: vi.fn(),
    startBookmark: vi.fn(),
    openHelp: vi.fn(),
  };
  const ctx: ShortcutContext = {
    vfoCtl: {
      getSnapshot: () => ({ vfos, selectedId, busy: false, error: null, loaded: true }),
      select: calls.select,
      update: calls.update,
      remove: calls.remove,
      create: calls.create,
    },
    audio: {
      getSnapshot: () => ({ levels: { 2: { listening: false } } }) as never,
      setListening: calls.setListening,
      getRecentLevels: () => [],
    },
    autoSquelch: { toggle: calls.toggle },
    step: "auto",
    zoom: calls.zoom,
    resetZoom: calls.resetZoom,
    createVfoAt: calls.createVfoAt,
    centerHz: 100_000_000,
    startBookmark: calls.startBookmark,
    openHelp: calls.openHelp,
    ...patch,
  };
  return { ctx, calls };
}

afterEach(() => toasts.clear());

describe("key map", () => {
  it("maps the documented keys", () => {
    expect(shortcutFor(key("ArrowLeft"))).toEqual({ type: "step", steps: -1 });
    expect(shortcutFor(key("ArrowRight"))).toEqual({ type: "step", steps: 1 });
    expect(shortcutFor(key("ArrowLeft", { shiftKey: true }))).toEqual({ type: "step", steps: -10 });
    expect(shortcutFor(key("ArrowRight", { shiftKey: true }))).toEqual({ type: "step", steps: 10 });
    expect(shortcutFor(key("ArrowUp"))).toEqual({ type: "vfo", delta: 1 });
    expect(shortcutFor(key("ArrowDown"))).toEqual({ type: "vfo", delta: -1 });
    expect(shortcutFor(key("1"))).toMatchObject({ type: "mode", mode: "wfm" });
    expect(shortcutFor(key("3"))).toMatchObject({ type: "mode", mode: "am" });
    expect(shortcutFor(key("4"))).toMatchObject({ type: "mode", mode: "usb", label: "USB" });
    expect(shortcutFor(key("5"))).toMatchObject({ type: "mode", mode: "lsb", label: "LSB" });
    expect(shortcutFor(key("6"))).toMatchObject({ type: "mode", mode: "cw", label: "CW" });
    expect(shortcutFor(key("m"))).toEqual({ type: "listen" });
    expect(shortcutFor(key("S", { shiftKey: true }))).toEqual({ type: "autoSquelch" });
    expect(shortcutFor(key("b"))).toEqual({ type: "bookmark" });
    expect(shortcutFor(key("+", { shiftKey: true }))).toEqual({ type: "zoom", factor: 0.5 });
    expect(shortcutFor(key("-"))).toEqual({ type: "zoom", factor: 2 });
    expect(shortcutFor(key("0"))).toEqual({ type: "zoomReset" });
    expect(shortcutFor(key("n"))).toEqual({ type: "newVfo" });
    expect(shortcutFor(key("Delete"))).toEqual({ type: "delete" });
    expect(shortcutFor(key("?", { shiftKey: true }))).toEqual({ type: "help" });
  });

  it("leaves unmapped keys and browser shortcuts alone", () => {
    expect(shortcutFor(key("x"))).toBeNull();
    expect(shortcutFor(key("7"))).toBeNull();
    expect(shortcutFor(key("s", { ctrlKey: true }))).toBeNull();
    expect(shortcutFor(key("ArrowLeft", { altKey: true }))).toBeNull();
    expect(shortcutFor(key("0", { metaKey: true }))).toBeNull();
  });
});

describe("input focus", () => {
  it("ignores keys aimed at fields and widgets with their own keys", () => {
    const input = document.createElement("input");
    const select = document.createElement("select");
    const slider = document.createElement("div");
    slider.setAttribute("role", "slider");
    const plain = document.createElement("div");
    expect(ignoresShortcuts(input)).toBe(true);
    expect(ignoresShortcuts(select)).toBe(true);
    expect(ignoresShortcuts(slider)).toBe(true);
    expect(ignoresShortcuts(plain)).toBe(false);
    expect(ignoresShortcuts(document.body)).toBe(false);
  });

  it("ignores everything while a modal dialog is open", () => {
    const modal = document.createElement("div");
    modal.setAttribute("aria-modal", "true");
    document.body.append(modal);
    expect(ignoresShortcuts(document.body)).toBe(true);
    modal.remove();
  });

  it("does not fire while typing, but does from the page", () => {
    const { ctx, calls } = context([vfo(1)], 1);
    function Harness() {
      useShortcuts(ctx);
      return <input aria-label="field" />;
    }
    render(<Harness />);
    const field = screen.getByLabelText("field");
    for (const k of ["n", "0", "m"]) fireEvent.keyDown(field, { key: k });
    expect(calls.createVfoAt).not.toHaveBeenCalled();
    expect(calls.resetZoom).not.toHaveBeenCalled();
    expect(calls.setListening).not.toHaveBeenCalled();
    for (const k of ["n", "0"]) fireEvent.keyDown(document.body, { key: k });
    expect(calls.createVfoAt).toHaveBeenCalledWith(100_000_000);
    expect(calls.resetZoom).toHaveBeenCalled();
  });
});

describe("actions", () => {
  const vfos = [vfo(1), vfo(2, { mode: "nfm", freqHz: 145_500_000 }), vfo(3)];

  it("steps the active VFO by its mode step, ten times for Shift", () => {
    const { ctx, calls } = context(vfos, 2);
    runShortcut({ type: "step", steps: 1 }, ctx);
    expect(calls.update).toHaveBeenLastCalledWith(2, { freqHz: 145_512_500 });
    runShortcut({ type: "step", steps: -10 }, ctx);
    expect(calls.update).toHaveBeenLastCalledWith(2, { freqHz: 145_375_000 });
  });

  it("uses the chosen step instead of the mode default", () => {
    const { ctx, calls } = context(vfos, 2, { step: 1_000 });
    runShortcut({ type: "step", steps: 1 }, ctx);
    expect(calls.update).toHaveBeenLastCalledWith(2, { freqHz: 145_501_000 });
  });

  it("cycles through the VFOs with wrap-around", () => {
    const { ctx, calls } = context(vfos, 3);
    runShortcut({ type: "vfo", delta: 1 }, ctx);
    expect(calls.select).toHaveBeenLastCalledWith(1);
    const first = context(vfos, 1);
    runShortcut({ type: "vfo", delta: -1 }, first.ctx);
    expect(first.calls.select).toHaveBeenLastCalledWith(3);
  });

  it("does nothing VFO-specific without an active VFO", () => {
    const { ctx, calls } = context([], null);
    expect(runShortcut({ type: "step", steps: 1 }, ctx)).toBe(false);
    expect(runShortcut({ type: "delete" }, ctx)).toBe(false);
    expect(runShortcut({ type: "listen" }, ctx)).toBe(false);
    expect(calls.update).not.toHaveBeenCalled();
  });

  it("changes the mode and resets the bandwidth", () => {
    const { ctx, calls } = context(vfos, 1);
    runShortcut({ type: "mode", mode: "am", label: "AM" }, ctx);
    expect(calls.update).toHaveBeenLastCalledWith(1, { mode: "am", bandwidthHz: null });
    calls.update.mockClear();
    runShortcut({ type: "mode", mode: "usb", label: "USB" }, ctx);
    expect(calls.update).toHaveBeenLastCalledWith(1, { mode: "usb", bandwidthHz: null });
  });

  it("toggles listening from the current state", () => {
    const { ctx, calls } = context(vfos, 2);
    runShortcut({ type: "listen" }, ctx);
    expect(calls.setListening).toHaveBeenCalledWith(2, true);
    const other = context(vfos, 1);
    runShortcut({ type: "listen" }, other.ctx);
    expect(other.calls.setListening).toHaveBeenCalledWith(1, false);
  });

  it("deletes the active VFO and offers to undo it", () => {
    const { ctx, calls } = context(vfos, 2);
    runShortcut({ type: "delete" }, ctx);
    expect(calls.remove).toHaveBeenCalledWith(2);
    const toast = toasts.getSnapshot().find((t) => t.key === "vfo-removed");
    expect(toast?.action?.label).toBe("Undo");
    toast?.action?.onClick();
    expect(calls.create).toHaveBeenCalledWith({
      freqHz: 145_500_000,
      mode: "nfm",
      bandwidthHz: 180_000,
      squelchDbfs: null,
    });
  });

  it("forwards zoom, bookmark, new VFO and help", () => {
    const { ctx, calls } = context(vfos, 1);
    runShortcut({ type: "zoom", factor: 0.5 }, ctx);
    runShortcut({ type: "bookmark" }, ctx);
    runShortcut({ type: "autoSquelch" }, ctx);
    runShortcut({ type: "help" }, ctx);
    expect(calls.zoom).toHaveBeenCalledWith(0.5);
    expect(calls.startBookmark).toHaveBeenCalled();
    expect(calls.toggle).toHaveBeenCalled();
    expect(calls.openHelp).toHaveBeenCalled();
  });
});
