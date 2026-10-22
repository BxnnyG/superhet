import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ApiError, type DeviceState, type LockHolder } from "../spectrum/api";
import { TuningControls } from "../spectrum/TuningControls";
import type { TuningSnapshot } from "../spectrum/tuning";
import { toasts } from "../ui";
import type { Vfo } from "../vfo/api";
import { noticeLocked, noticeRetune } from "./notices";
import { ReceiverFrequency } from "./ReceiverFrequency";
import { TuningLock } from "./TuningLock";

const user: LockHolder = { kind: "user", id: "user", sinceMs: 1 };
const rec: LockHolder = { kind: "recording", id: "r", sinceMs: 2 };

beforeEach(() => toasts.clear());
afterEach(() => toasts.clear());

it("TuningLock: unlocked state offers to lock", () => {
  const onToggle = vi.fn();
  render(<TuningLock holders={[]} onToggle={onToggle} />);
  const b = screen.getByRole("button", { name: "Lock tuning" });
  expect(b).toHaveAttribute("aria-pressed", "false");
  fireEvent.click(b);
  expect(onToggle).toHaveBeenCalledWith(true);
  expect(screen.queryByText(/Locked by/)).toBeNull();
});

it("TuningLock: shows 'Locked by you' and unlocks", () => {
  const onToggle = vi.fn();
  render(<TuningLock holders={[user]} onToggle={onToggle} />);
  expect(screen.getByText("Locked by you")).toBeInTheDocument();
  const b = screen.getByRole("button", { name: "Unlock tuning" });
  expect(b).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(b);
  expect(onToggle).toHaveBeenCalledWith(false);
});

it("TuningLock: names other holders", () => {
  render(<TuningLock holders={[user, rec]} onToggle={() => undefined} />);
  expect(screen.getByText("Locked by you and a recording")).toBeInTheDocument();
});

it("TuningLock: disabled while a request runs", () => {
  render(<TuningLock holders={[]} disabled onToggle={() => undefined} />);
  expect(screen.getByRole("button", { name: "Lock tuning" })).toBeDisabled();
});

const vfo: Vfo = {
  id: 2,
  freqHz: 145_500_000,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
};

it("ReceiverFrequency: the centre readout is read-only while locked, VFOs stay editable", () => {
  const onCenter = vi.fn();
  const onVfo = vi.fn();
  const { rerender } = render(
    <ReceiverFrequency
      centerHz={100_000_000}
      selected={null}
      onCenter={onCenter}
      onVfo={onVfo}
      centerLockedReason="Locked by you"
    />,
  );
  const digit = screen.getAllByRole("spinbutton")[0]!;
  expect(digit).toHaveAttribute("aria-disabled", "true");
  fireEvent.keyDown(digit, { key: "ArrowUp" });
  expect(onCenter).not.toHaveBeenCalled();
  expect(screen.getByRole("button", { name: "Type a frequency" })).toBeDisabled();
  rerender(
    <ReceiverFrequency
      centerHz={100_000_000}
      selected={vfo}
      onCenter={onCenter}
      onVfo={onVfo}
      centerLockedReason="Locked by you"
    />,
  );
  fireEvent.keyDown(screen.getAllByRole("spinbutton")[6]!, { key: "ArrowUp" });
  expect(onVfo).toHaveBeenCalled();
});

it("ReceiverFrequency: badge marks a paused VFO", () => {
  render(
    <ReceiverFrequency
      centerHz={100e6}
      selected={{ ...vfo, status: "paused" }}
      onCenter={() => undefined}
      onVfo={() => undefined}
    />,
  );
  expect(screen.getByText(/VFO 2 · NFM/)).toHaveTextContent("paused");
});

const state: DeviceState = {
  driver: "rtlsdr",
  centerHz: 100e6,
  sampleRate: 2_048_000,
  gain: { mode: "auto", db: null },
  gainsDb: [0, 10],
  ppm: 0,
  biasTee: false,
  tuningEpoch: 1,
  locks: [],
  window: { startHz: 99e6, endHz: 101e6 },
};

function snap(locks: LockHolder[]): TuningSnapshot {
  const s = { ...state, locks };
  return { state: s, display: s, busy: false, lockBusy: false, error: null };
}

it("TuningControls: centre and rate are disabled with the reason while locked", () => {
  const { rerender } = render(<TuningControls tuning={snap([])} onTune={() => undefined} />);
  expect(screen.getByLabelText("Centre frequency")).toBeEnabled();
  expect(screen.getByLabelText("Sample rate")).toBeEnabled();
  expect(screen.queryByText(/Unlock to change/)).toBeNull();
  rerender(<TuningControls tuning={snap([user])} onTune={() => undefined} />);
  expect(screen.getByLabelText("Centre frequency")).toBeDisabled();
  expect(screen.getByLabelText("Sample rate")).toBeDisabled();
  expect(screen.getByText(/Locked by you\. Unlock to change/)).toBeInTheDocument();
  expect(screen.getByLabelText("Frequency correction")).toBeEnabled();
});

it("a rejected tune (409 locked) raises one non-blocking toast listing the holders", () => {
  const e = new ApiError("tuning is locked by: user", 409, "locked", [user, rec]);
  noticeLocked(e.holders ?? []);
  noticeLocked(e.holders ?? []);
  const items = toasts.getSnapshot();
  expect(items).toHaveLength(1);
  expect(items[0]).toMatchObject({ tone: "warn", title: "Tuning is locked" });
  expect(items[0]?.body).toContain("Locked by you and a recording");
});

it("free tuning: locked retune hints that the VFO is paused, failure is an error", () => {
  noticeRetune("locked");
  expect(toasts.getSnapshot()[0]?.title).toBe("Tuning locked — VFO paused outside the window");
  noticeRetune("failed");
  expect(toasts.getSnapshot()).toHaveLength(1);
  expect(toasts.getSnapshot()[0]).toMatchObject({ tone: "danger" });
});
