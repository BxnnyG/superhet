import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { ReceiverFrequency } from "./ReceiverFrequency";

it("tunes the device centre", () => {
  const onCenter = vi.fn();
  render(<ReceiverFrequency centerHz={100_000_000} onCenter={onCenter} />);
  expect(screen.getByRole("group", { name: "Centre frequency" })).toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("spinbutton", { name: "100 kHz digit" }), {
    key: "ArrowUp",
  });
  expect(onCenter).toHaveBeenCalledWith(100_100_000);
});

it("typed entry goes to the centre", () => {
  const onCenter = vi.fn();
  render(<ReceiverFrequency centerHz={100_000_000} onCenter={onCenter} />);
  fireEvent.keyDown(screen.getByRole("spinbutton", { name: "1 kHz digit" }), { key: "Enter" });
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "101.5" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(onCenter).toHaveBeenCalledWith(101_500_000);
});
