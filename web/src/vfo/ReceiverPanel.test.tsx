import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import { toasts } from "../ui";
import type { Vfo } from "./api";
import type { VfoSnapshot } from "./controller";
import { ReceiverDock } from "./ReceiverDock";

const vfo = (id: number, over: Partial<Vfo> = {}): Vfo => ({
  id,
  freqHz: 100e6 + id * 1e5,
  mode: "nfm",
  bandwidthHz: 12_500,
  squelchDbfs: null,
  deemphasis: null,
  offsetHz: 0,
  status: "active",
  ...over,
});

const recent = vi.fn<(id: number, windowMs: number) => number[]>(() => []);
const audio = {
  getMeter: () => null,
  getRecentLevels: recent,
  setListening: vi.fn(),
  setVolume: vi.fn(),
  setMuted: vi.fn(),
  enable: vi.fn(),
  disable: vi.fn(),
} as unknown as AudioController;

const audioSnap = {
  levels: {
    1: { volume: 0.8, muted: false, listening: false },
    2: { volume: 0.5, muted: false, listening: true },
  },
  enabled: false,
  starting: false,
  unsupported: null,
  rds: {},
  tones: {},
} as unknown as AudioSnapshot;

beforeEach(() => {
  vi.clearAllMocks();
  toasts.clear();
});

function setup(
  vfos: Vfo[],
  selectedId: number | null,
  loaded = true,
  snapshot: AudioSnapshot = audioSnap,
) {
  const snap: VfoSnapshot = { vfos, selectedId, busy: false, error: null, loaded };
  const fns = {
    onCreate: vi.fn(),
    onSelect: vi.fn(),
    onUpdate: vi.fn(),
    onRemove: vi.fn(),
  };
  render(<ReceiverDock vfos={snap} audio={audio} audioSnap={snapshot} {...fns} />);
  return fns;
}

it("shows the active VFO: chips, mode, squelch and volume (the frequency lives on top)", () => {
  setup([vfo(1), vfo(2, { mode: "am", bandwidthHz: 10_000 })], 2);
  expect(screen.getByRole("button", { name: /^VFO 1 NFM/ })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
  expect(screen.getByRole("button", { name: /^VFO 2 AM/ })).toHaveAttribute("aria-pressed", "true");
  expect(screen.queryByRole("group", { name: "VFO frequency" })).toBeNull();
  expect(screen.queryByRole("spinbutton")).toBeNull();
  expect(screen.getByRole("radio", { name: "AM" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getByRole("slider", { name: "Volume" })).toHaveValue("50");
  expect(screen.getByRole("slider", { name: "Squelch" })).toBeInTheDocument();
});

it("a mode change updates the active VFO and resets its bandwidth", () => {
  const fns = setup([vfo(1), vfo(2)], 2);
  fireEvent.click(screen.getByRole("radio", { name: "WFM" }));
  expect(fns.onUpdate).toHaveBeenCalledWith(2, { mode: "wfm", bandwidthHz: null });
});

it("selects a VFO from its chip and adds a new one", () => {
  const fns = setup([vfo(1), vfo(2)], 1);
  fireEvent.click(screen.getByRole("button", { name: /^VFO 2/ }));
  expect(fns.onSelect).toHaveBeenCalledWith(2);
  fireEvent.click(screen.getByRole("button", { name: "Add VFO" }));
  expect(fns.onCreate).toHaveBeenCalled();
});

it("marks a paused active VFO", () => {
  setup([vfo(1, { status: "paused" })], 1);
  expect(screen.getByText("paused")).toBeInTheDocument();
});

it("shows the empty state hint and a + button when there are no VFOs", () => {
  const fns = setup([], null);
  expect(screen.queryByRole("radiogroup", { name: "Mode" })).toBeNull();
  expect(screen.getByText("Click the spectrum or press N to add a VFO")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Add VFO" }));
  expect(fns.onCreate).toHaveBeenCalled();
});

it("each chip has a listen toggle reflecting and changing whether the VFO is in the mix", () => {
  setup([vfo(1), vfo(2)], 2);
  const one = screen.getByRole("button", { name: "Listen to VFO 1" });
  const two = screen.getByRole("button", { name: "Listen to VFO 2" });
  expect(one).toHaveAttribute("aria-pressed", "false");
  expect(two).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(one);
  expect(audio.setListening).toHaveBeenCalledWith(1, true);
  fireEvent.click(two);
  expect(audio.setListening).toHaveBeenCalledWith(2, false);
});

it("selecting a VFO does not change whether it is listened to", () => {
  const fns = setup([vfo(1), vfo(2)], 2);
  fireEvent.click(screen.getByRole("button", { name: /^VFO 1 / }));
  expect(fns.onSelect).toHaveBeenCalledWith(1);
  expect(audio.setListening).not.toHaveBeenCalled();
});

it("Auto squelch applies the noise floor of the last ~3 s, shows auto and clears on a second click", () => {
  recent.mockReturnValue([-90, -89, -88, -87, -86, -85, -84, -83, -82, -50]);
  const snap = (squelchDbfs: number | null): VfoSnapshot => ({
    vfos: [vfo(2, { squelchDbfs })],
    selectedId: 2,
    busy: false,
    error: null,
    loaded: true,
  });
  const onUpdate = vi.fn();
  const props = {
    audio,
    audioSnap,
    onCreate: vi.fn(),
    onSelect: vi.fn(),
    onUpdate,
    onRemove: vi.fn(),
  };
  const { rerender } = render(<ReceiverDock vfos={snap(null)} {...props} />);
  const auto = screen.getByRole("button", { name: "Auto" });
  expect(auto).toHaveAttribute("aria-pressed", "false");
  fireEvent.click(auto);
  expect(recent).toHaveBeenCalledWith(2, 3000);
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: -85 });
  rerender(<ReceiverDock vfos={snap(-85)} {...props} />);
  expect(screen.getByRole("button", { name: "Auto" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByText("auto -85 dBFS")).toBeInTheDocument();
  // Moving the slider leaves auto mode.
  fireEvent.change(screen.getByRole("slider", { name: "Squelch" }), { target: { value: "-70" } });
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: -70 });
  rerender(<ReceiverDock vfos={snap(-70)} {...props} />);
  expect(screen.getByRole("button", { name: "Auto" })).toHaveAttribute("aria-pressed", "false");
  // Auto again, then a second click clears the squelch.
  fireEvent.click(screen.getByRole("button", { name: "Auto" }));
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: -85 });
  rerender(<ReceiverDock vfos={snap(-85)} {...props} />);
  fireEvent.click(screen.getByRole("button", { name: "Auto" }));
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: null });
});

it("Auto squelch without enough signal data only tells the user", () => {
  recent.mockReturnValue([-80, -80]);
  const fns = setup([vfo(2)], 2);
  fireEvent.click(screen.getByRole("button", { name: "Auto" }));
  expect(fns.onUpdate).not.toHaveBeenCalled();
  expect(toasts.getSnapshot()[0]?.title).toBe("Not enough signal data yet");
});

const wfm = (over: Partial<Vfo> = {}) =>
  vfo(1, { mode: "wfm", bandwidthHz: 180_000, deemphasis: "50us", stereo: true, ...over });

it("offers the stereo switch for WFM only and toggles it", () => {
  const { onUpdate } = setup([wfm()], 1);
  const toggle = screen.getByRole("button", { name: "Stereo" });
  expect(toggle).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(toggle);
  expect(onUpdate).toHaveBeenCalledWith(1, { stereo: false });
  expect(screen.getByTestId("stereo-lamp")).toBeInTheDocument();
});

it("shows the switch as off when stereo is disabled", () => {
  const { onUpdate } = setup([wfm({ stereo: false })], 1);
  const toggle = screen.getByRole("button", { name: "Stereo" });
  expect(toggle).toHaveAttribute("aria-pressed", "false");
  fireEvent.click(toggle);
  expect(onUpdate).toHaveBeenCalledWith(1, { stereo: true });
});

it("has no stereo switch and no RDS row for narrow-band modes", () => {
  setup([vfo(1)], 1);
  expect(screen.queryByRole("button", { name: "Stereo" })).toBeNull();
  expect(screen.queryByRole("group", { name: "RDS" })).toBeNull();
});

it("shows RDS station name, programme type, PI and RadioText of a WFM VFO", () => {
  const snapshot = {
    ...audioSnap,
    rds: {
      1: {
        synced: true,
        pi: "D3C2",
        pty: 10,
        tp: true,
        ta: false,
        ms: true,
        ps: "SUPERHET",
        rt: "Now playing something",
      },
    },
  } as unknown as AudioSnapshot;
  setup([wfm()], 1, true, snapshot);
  expect(screen.getByTestId("rds-ps")).toHaveTextContent("SUPERHET");
  expect(screen.getByText("Pop music")).toBeInTheDocument();
  expect(screen.getByText("PI D3C2")).toBeInTheDocument();
  expect(screen.getByText("TP")).toBeInTheDocument();
  expect(screen.queryByText("TA")).toBeNull();
  expect(screen.getByTestId("rds-rt")).toHaveTextContent("Now playing something");
  expect(screen.queryByText("signal lost")).toBeNull();
});

it("says so when a WFM VFO has no RDS data, and when the signal was lost", () => {
  setup([wfm()], 1);
  expect(screen.getByText("no data")).toBeInTheDocument();
});

it("marks stale RDS data when the signal is lost", () => {
  const snapshot = {
    ...audioSnap,
    rds: {
      1: {
        synced: false,
        pi: "D3C2",
        pty: null,
        tp: null,
        ta: null,
        ms: null,
        ps: "SUPERHET",
        rt: null,
      },
    },
  } as unknown as AudioSnapshot;
  setup([wfm()], 1, true, snapshot);
  expect(screen.getByText("signal lost")).toBeInTheDocument();
});
