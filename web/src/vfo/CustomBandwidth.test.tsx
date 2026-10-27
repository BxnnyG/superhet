import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { CustomBandwidth, validateBandwidthInput } from "./CustomBandwidth";

function open(onApply = vi.fn(), sampleRateHz: number | null = 2_400_000) {
  render(
    <CustomBandwidth mode="nfm" value={12_500} sampleRateHz={sampleRateHz} onApply={onApply} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Custom bandwidth" }));
  return { onApply, input: screen.getByRole("textbox", { name: "Bandwidth" }) };
}

it("applies a typed width in kHz and closes", () => {
  const { onApply, input } = open();
  expect(input).toHaveValue("12.5");
  fireEvent.change(input, { target: { value: "13.337" } });
  expect(screen.getByText("= 13.34 kHz")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Apply" }));
  expect(onApply).toHaveBeenCalledWith(13_337);
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("accepts an exact value in Hz", () => {
  const { onApply, input } = open();
  fireEvent.change(input, { target: { value: "8500 Hz" } });
  fireEvent.submit(input.closest("form") as HTMLFormElement);
  expect(onApply).toHaveBeenCalledWith(8_500);
});

it("blocks values outside the mode range and says why", () => {
  const { onApply, input } = open();
  fireEvent.change(input, { target: { value: "500 Hz" } });
  expect(screen.getByText("At least 1 kHz in this mode.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
  fireEvent.submit(input.closest("form") as HTMLFormElement);
  expect(onApply).not.toHaveBeenCalled();
});

it("blocks widths the device sample rate cannot carry", () => {
  const { input } = open(vi.fn(), 250_000);
  fireEvent.change(input, { target: { value: "300" } });
  expect(screen.getByText("At most 208.3 kHz at this sample rate.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Apply" })).toBeDisabled();
});

it("an unparsable entry shows a format hint, an empty one just stays disabled", () => {
  expect(validateBandwidthInput("wide", "nfm")).toEqual({
    hz: null,
    error: "Enter a width such as 12.5 kHz or 8500 Hz.",
  });
  expect(validateBandwidthInput("  ", "nfm")).toEqual({ hz: null, error: null });
  expect(validateBandwidthInput("25", "nfm")).toEqual({ hz: 25_000, error: null });
});

it("Escape closes without applying", () => {
  const { onApply } = open();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(onApply).not.toHaveBeenCalled();
});
