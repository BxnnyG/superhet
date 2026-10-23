import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import type { ToneState } from "../audio/tones";
import { toasts } from "../ui";
import type { Vfo } from "./api";
import { TonesPanel } from "./TonesPanel";

const vfo = (over: Partial<Vfo> = {}): Vfo => ({
  id: 3,
  freqHz: 446_006_250,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  tones: true,
  toneSquelchHz: null,
  offsetHz: 0,
  status: "active",
  ...over,
});

const state = (over: Partial<ToneState> = {}): ToneState => ({
  ctcssHz: null,
  digits: "",
  count: 0,
  ...over,
});

function setup(v: Vfo, tones?: ToneState) {
  const audio = { clearDigits: vi.fn() };
  const onUpdate = vi.fn();
  render(<TonesPanel vfo={v} tones={tones} audio={audio} onUpdate={onUpdate} />);
  return { audio, onUpdate };
}

beforeEach(() => toasts.clear());

it("shows the detected sub-tone and the digits", () => {
  setup(vfo(), state({ ctcssHz: 88.5, digits: "12*A", count: 4 }));
  expect(screen.getByTestId("ctcss")).toHaveTextContent("88.5 Hz");
  expect(screen.getByTestId("dtmf-digits")).toHaveTextContent("12*A");
});

it("shows placeholders when nothing was detected", () => {
  setup(vfo());
  expect(screen.getByTestId("ctcss")).toHaveTextContent("none");
  expect(screen.getByRole("button", { name: "Copy" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Clear" })).toBeDisabled();
});

it("the switch turns detection on and off together with the tone squelch", () => {
  const { onUpdate } = setup(vfo({ toneSquelchHz: 88.5 }));
  fireEvent.click(screen.getByRole("switch", { name: "Detect CTCSS and DTMF" }));
  expect(onUpdate).toHaveBeenCalledWith(3, { tones: false, toneSquelchHz: null });
});

it("with detection off only the switch is offered", () => {
  const { onUpdate } = setup(vfo({ tones: false }));
  expect(screen.queryByRole("button", { name: "Copy" })).toBeNull();
  expect(screen.queryByRole("combobox", { name: "Tone squelch" })).toBeNull();
  fireEvent.click(screen.getByRole("switch", { name: "Detect CTCSS and DTMF" }));
  expect(onUpdate).toHaveBeenCalledWith(3, { tones: true });
});

it("selects a tone squelch, switches it off and adopts the detected tone", () => {
  const { onUpdate } = setup(vfo({ toneSquelchHz: 100 }), state({ ctcssHz: 88.5 }));
  const select = screen.getByRole("combobox", { name: "Tone squelch" });
  expect(select).toHaveValue("100");
  fireEvent.change(select, { target: { value: "123" } });
  expect(onUpdate).toHaveBeenLastCalledWith(3, { toneSquelchHz: 123 });
  fireEvent.change(select, { target: { value: "off" } });
  expect(onUpdate).toHaveBeenLastCalledWith(3, { toneSquelchHz: null });
  fireEvent.click(screen.getByRole("button", { name: "Use" }));
  expect(onUpdate).toHaveBeenLastCalledWith(3, { toneSquelchHz: 88.5 });
});

it("clears and copies the digits", async () => {
  const writeText = vi.fn().mockResolvedValue(undefined);
  Object.assign(navigator, { clipboard: { writeText } });
  const { audio } = setup(vfo(), state({ digits: "4711", count: 4 }));
  fireEvent.click(screen.getByRole("button", { name: "Clear" }));
  expect(audio.clearDigits).toHaveBeenCalledWith(3);
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  expect(writeText).toHaveBeenCalledWith("4711");
  await vi.waitFor(() => expect(toasts.getSnapshot()[0]).toMatchObject({ title: "Digits copied" }));
});

it("tells the user when the clipboard is not available", async () => {
  Object.assign(navigator, {
    clipboard: { writeText: vi.fn().mockRejectedValue(new Error("no")) },
  });
  setup(vfo(), state({ digits: "1", count: 1 }));
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  await vi.waitFor(() => expect(toasts.getSnapshot()[0]).toMatchObject({ tone: "warn" }));
});
