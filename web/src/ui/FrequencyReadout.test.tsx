import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { expect, it, vi } from "vitest";
import { FrequencyReadout } from "./FrequencyReadout";

function Controlled({
  initial,
  onChange,
  ...extra
}: { initial: number | null; onChange: (hz: number) => void } & Partial<
  Parameters<typeof FrequencyReadout>[0]
>) {
  const [hz, setHz] = useState(initial);
  return (
    <FrequencyReadout
      label="Centre frequency"
      valueHz={hz}
      onChange={(v) => {
        setHz(v);
        onChange(v);
      }}
      {...extra}
    />
  );
}

/** Renders a parent that feeds the new value back, like the receiver does. */
function setup(
  valueHz: number | null,
  extra: Partial<Parameters<typeof FrequencyReadout>[0]> = {},
) {
  const onChange = vi.fn();
  render(<Controlled initial={valueHz} onChange={onChange} {...extra} />);
  return onChange;
}

const digit = (name: string) => screen.getByRole("spinbutton", { name });

it("shows grouped digits with dimmed leading zeros", () => {
  setup(145_500_000);
  const digits = screen.getAllByRole("spinbutton").map((d) => d.textContent);
  expect(digits.join("")).toBe("0145500000");
  expect(digit("100 MHz digit")).toHaveAttribute("aria-valuenow", "1");
  expect(digit("1000 MHz digit").className).toContain("text-fg-subtle");
});

it("wheel up/down changes the digit under the pointer", () => {
  const onChange = setup(145_500_000);
  fireEvent.wheel(digit("1 kHz digit"), { deltaY: -100 });
  expect(onChange).toHaveBeenLastCalledWith(145_501_000);
  fireEvent.wheel(digit("100 kHz digit"), { deltaY: 100 });
  expect(onChange).toHaveBeenLastCalledWith(145_401_000);
});

it("wheel on a digit prevents page scrolling", () => {
  setup(145_500_000);
  const notPrevented = fireEvent.wheel(digit("1 MHz digit"), { deltaY: -100 });
  expect(notPrevented).toBe(false);
});

it("small wheel deltas accumulate to one step", () => {
  const onChange = setup(145_500_000);
  const d = digit("10 kHz digit");
  fireEvent.wheel(d, { deltaY: -20 });
  fireEvent.wheel(d, { deltaY: -20 });
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.wheel(d, { deltaY: -20 });
  expect(onChange).toHaveBeenCalledTimes(1);
  expect(onChange).toHaveBeenLastCalledWith(145_510_000);
});

it("ArrowUp/ArrowDown step the focused digit, arrows left/right move focus", () => {
  const onChange = setup(145_500_000);
  const khz = digit("1 kHz digit");
  khz.focus();
  fireEvent.keyDown(khz, { key: "ArrowUp" });
  expect(onChange).toHaveBeenLastCalledWith(145_501_000);
  fireEvent.keyDown(khz, { key: "ArrowRight" });
  expect(digit("100 Hz digit")).toHaveFocus();
  fireEvent.keyDown(digit("100 Hz digit"), { key: "ArrowDown" });
  expect(onChange).toHaveBeenLastCalledWith(145_500_900);
  fireEvent.keyDown(digit("100 Hz digit"), { key: "ArrowLeft" });
  expect(khz).toHaveFocus();
});

it("carries across digits", () => {
  const onChange = setup(145_999_000);
  fireEvent.keyDown(digit("1 kHz digit"), { key: "ArrowUp" });
  expect(onChange).toHaveBeenLastCalledWith(146_000_000);
});

it("clamps at the limits", () => {
  const onChange = setup(500_000, { minHz: 500_000, maxHz: 1_000_000 });
  fireEvent.keyDown(digit("1 kHz digit"), { key: "ArrowDown" });
  expect(onChange).toHaveBeenLastCalledWith(500_000);
  fireEvent.keyDown(digit("100 kHz digit"), { key: "ArrowUp" });
  fireEvent.keyDown(digit("1 MHz digit"), { key: "ArrowUp" });
  expect(onChange).toHaveBeenLastCalledWith(1_000_000);
});

it("typing a digit overwrites it and moves right", () => {
  const onChange = setup(145_500_000);
  const d = digit("10 kHz digit");
  d.focus();
  fireEvent.keyDown(d, { key: "7" });
  expect(onChange).toHaveBeenLastCalledWith(145_570_000);
  expect(digit("1 kHz digit")).toHaveFocus();
});

it("Enter switches to typed entry and Enter commits a parsed value", () => {
  const onChange = setup(145_500_000);
  fireEvent.keyDown(digit("1 kHz digit"), { key: "Enter" });
  const input = screen.getByRole("textbox", { name: /centre frequency/i });
  fireEvent.change(input, { target: { value: "433.92" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(onChange).toHaveBeenLastCalledWith(433_920_000);
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
});

it("rejects unparsable input and Escape cancels", () => {
  const onChange = setup(145_500_000);
  fireEvent.doubleClick(screen.getByRole("group", { name: "Centre frequency" }));
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "abc" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(input).toHaveAttribute("aria-invalid", "true");
  expect(onChange).not.toHaveBeenCalled();
  fireEvent.keyDown(input, { key: "Escape" });
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
});

it("is inert without a value", () => {
  const onChange = setup(null);
  fireEvent.keyDown(digit("1 kHz digit"), { key: "ArrowUp" });
  fireEvent.wheel(digit("1 kHz digit"), { deltaY: -100 });
  expect(onChange).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: /type a frequency/i })).toBeDisabled();
});
