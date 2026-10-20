import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { expect, it, vi } from "vitest";
import { Badge } from "./Badge";
import { Button } from "./Button";
import { IconButton } from "./IconButton";
import { Meter } from "./Meter";
import { NumberField } from "./NumberField";
import { Section } from "./Section";
import { SegmentedControl } from "./SegmentedControl";
import { Select } from "./Select";
import { Slider, markerLeft } from "./Slider";
import { Toggle } from "./Toggle";
import { Tooltip } from "./Tooltip";

it("Button renders variants, forwards clicks and respects disabled", () => {
  const onClick = vi.fn();
  const { rerender } = render(
    <Button variant="primary" onClick={onClick}>
      Go
    </Button>,
  );
  const b = screen.getByRole("button", { name: "Go" });
  expect(b).toHaveAttribute("type", "button");
  fireEvent.click(b);
  expect(onClick).toHaveBeenCalledTimes(1);
  rerender(
    <Button variant="danger" disabled onClick={onClick}>
      Go
    </Button>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Go" }));
  expect(onClick).toHaveBeenCalledTimes(1);
});

it("IconButton is named by its label and exposes pressed state", () => {
  render(
    <IconButton label="Mute" pressed>
      <svg />
    </IconButton>,
  );
  const b = screen.getByRole("button", { name: "Mute" });
  expect(b).toHaveAttribute("aria-pressed", "true");
});

it("Tooltip text is decorative", () => {
  render(
    <Tooltip text="Hint">
      <span>x</span>
    </Tooltip>,
  );
  expect(screen.getByText("Hint")).toHaveAttribute("aria-hidden", "true");
});

it("Select reports the chosen value and has a label", () => {
  const onChange = vi.fn();
  render(
    <Select
      label="Rate"
      value="a"
      onChange={onChange}
      options={[
        { value: "a", label: "A" },
        { value: "b", label: "B" },
      ]}
    />,
  );
  fireEvent.change(screen.getByLabelText("Rate"), { target: { value: "b" } });
  expect(onChange).toHaveBeenCalledWith("b");
});

it("Slider is a labelled range with value text", () => {
  const onChange = vi.fn();
  render(
    <Slider label="Floor" min={-100} max={0} value={-40} valueLabel="-40 dB" onChange={onChange} />,
  );
  const s = screen.getByRole("slider", { name: "Floor" });
  expect(s).toHaveAttribute("aria-valuetext", "-40 dB");
  fireEvent.change(s, { target: { value: "-60" } });
  expect(onChange).toHaveBeenCalledWith(-60);
  expect(markerLeft(0.5)).toContain("0.5");
  expect(markerLeft(7)).toContain("* 1");
});

it("Toggle is a switch", () => {
  const onChange = vi.fn();
  render(<Toggle label="Auto" checked={false} onChange={onChange} />);
  const t = screen.getByRole("switch", { name: "Auto" });
  expect(t).toHaveAttribute("aria-checked", "false");
  fireEvent.click(t);
  expect(onChange).toHaveBeenCalledWith(true);
});

function Seg() {
  const [v, setV] = useState("wfm");
  return (
    <SegmentedControl
      aria-label="Mode"
      value={v}
      onChange={setV}
      options={[
        { value: "wfm", label: "WFM" },
        { value: "nfm", label: "NFM" },
        { value: "am", label: "AM", disabled: true },
      ]}
    />
  );
}

it("SegmentedControl uses radio semantics, roving tabindex and arrow keys (skipping disabled)", () => {
  render(<Seg />);
  const wfm = screen.getByRole("radio", { name: "WFM" });
  const nfm = screen.getByRole("radio", { name: "NFM" });
  expect(wfm).toHaveAttribute("aria-checked", "true");
  expect(wfm).toHaveAttribute("tabindex", "0");
  expect(nfm).toHaveAttribute("tabindex", "-1");
  fireEvent.keyDown(wfm, { key: "ArrowRight" });
  expect(nfm).toHaveAttribute("aria-checked", "true");
  expect(nfm).toHaveFocus();
  fireEvent.keyDown(nfm, { key: "ArrowRight" });
  expect(wfm).toHaveAttribute("aria-checked", "true");
});

it("NumberField commits clamped values and steps with arrows", () => {
  const onCommit = vi.fn();
  render(<NumberField label="PPM" unit="ppm" value={5} min={-10} max={10} onCommit={onCommit} />);
  const input = screen.getByLabelText("PPM");
  expect(input).toHaveValue("5");
  fireEvent.change(input, { target: { value: "50" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(onCommit).toHaveBeenLastCalledWith(10);
  fireEvent.keyDown(input, { key: "ArrowDown" });
  expect(onCommit).toHaveBeenLastCalledWith(4);
  fireEvent.change(input, { target: { value: "x" } });
  fireEvent.keyDown(input, { key: "Enter" });
  expect(input).toHaveAttribute("aria-invalid", "true");
});

it("Section collapses and keeps content mounted", () => {
  render(
    <Section title="Source">
      <p>content</p>
    </Section>,
  );
  const header = screen.getByRole("button", { name: "Source" });
  expect(header).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByText("content")).toBeVisible();
  fireEvent.click(header);
  expect(header).toHaveAttribute("aria-expanded", "false");
  expect(screen.getByText("content")).not.toBeVisible();
});

it("Badge and Meter render", () => {
  render(
    <>
      <Badge tone="success" dot>
        OK
      </Badge>
      <Meter value={0.5} aria-label="Level" marker={0.8} ticks={[{ at: 0, label: "0" }]} />
    </>,
  );
  expect(screen.getByText("OK")).toBeInTheDocument();
  expect(screen.getByRole("meter", { name: "Level" })).toHaveAttribute("aria-valuenow", "0.5");
});
