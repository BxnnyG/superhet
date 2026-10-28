import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import { PluginStore } from "../plugins/store";
import { aprs, fakePluginApi, pocsag } from "../plugins/testing";
import { toasts } from "../ui";
import type { Vfo, VfoPatch } from "./api";
import { AutoSquelch } from "./autoSquelch";
import { ReceiverPanel } from "./ReceiverPanel";

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
  autoSquelch = new AutoSquelch();
  toasts.clear();
});

interface Fns {
  onUpdate: (id: number, patch: VfoPatch) => void;
  onRemove: (id: number) => void;
}

const makeFns = () => ({
  onUpdate: vi.fn<Fns["onUpdate"]>(),
  onRemove: vi.fn<Fns["onRemove"]>(),
});

let autoSquelch = new AutoSquelch();

function panel(
  vfos: Vfo[],
  selectedId: number | null,
  fns: Fns,
  snapshot: AudioSnapshot = audioSnap,
  loaded = true,
) {
  const selected = vfos.find((v) => v.id === selectedId) ?? null;
  return (
    <ReceiverPanel
      vfo={selected}
      loaded={loaded}
      audio={audio}
      audioSnap={snapshot}
      autoSquelch={autoSquelch}
      locked={false}
      showDemodulator
      {...fns}
    />
  );
}

function setup(
  vfos: Vfo[],
  selectedId: number | null,
  loaded = true,
  snapshot: AudioSnapshot = audioSnap,
) {
  const fns = makeFns();
  render(panel(vfos, selectedId, fns, snapshot, loaded));
  return fns;
}

it("shows the selected VFO: mode, squelch and volume", () => {
  setup([vfo(1), vfo(2, { mode: "am", bandwidthHz: 10_000 })], 2);
  expect(screen.getByRole("radio", { name: "AM" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getByRole("slider", { name: "Volume" })).toHaveValue("50");
  expect(screen.getByRole("slider", { name: "Squelch" })).toBeInTheDocument();
  expect(screen.queryByRole("spinbutton")).toBeNull();
});

it("a mode change updates the active VFO and resets its bandwidth", () => {
  const fns = setup([vfo(1), vfo(2)], 2);
  fireEvent.click(screen.getByRole("radio", { name: "WFM" }));
  expect(fns.onUpdate).toHaveBeenCalledWith(2, { mode: "wfm", bandwidthHz: null });
});

it("marks a paused VFO", () => {
  setup([vfo(1, { status: "paused" })], 1);
  expect(screen.getByText("paused")).toBeInTheDocument();
});

it("explains how to add a receiver when there is none", () => {
  setup([], null);
  expect(screen.queryByRole("radiogroup", { name: "Mode" })).toBeNull();
  expect(screen.getByText("Click the spectrum or press N to add a VFO")).toBeInTheDocument();
});

it("removes the selected VFO", () => {
  const fns = setup([vfo(1), vfo(2)], 2);
  fireEvent.click(screen.getByRole("button", { name: "Remove VFO" }));
  expect(fns.onRemove).toHaveBeenCalledWith(2);
});

it("a volume change goes to the audio controller", () => {
  setup([vfo(2)], 2);
  fireEvent.change(screen.getByRole("slider", { name: "Volume" }), { target: { value: "30" } });
  expect(audio.setVolume).toHaveBeenCalledWith(2, 0.3);
});

it("Auto squelch applies the noise floor of the last ~3 s, shows auto and clears on a second click", () => {
  recent.mockReturnValue([-90, -89, -88, -87, -86, -85, -84, -83, -82, -50]);
  const withSquelch = (squelchDbfs: number | null) => [vfo(2, { squelchDbfs })];
  const fns = makeFns();
  const onUpdate = fns.onUpdate;
  const { rerender } = render(panel(withSquelch(null), 2, fns));
  const auto = screen.getByRole("button", { name: "Auto" });
  expect(auto).toHaveAttribute("aria-pressed", "false");
  fireEvent.click(auto);
  expect(recent).toHaveBeenCalledWith(2, 3000);
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: -85 });
  rerender(panel(withSquelch(-85), 2, fns));
  expect(screen.getByRole("button", { name: "Auto" })).toHaveAttribute("aria-pressed", "true");
  expect(screen.getByText("auto -85 dBFS")).toBeInTheDocument();
  // Moving the slider leaves auto mode.
  fireEvent.change(screen.getByRole("slider", { name: "Squelch" }), { target: { value: "-70" } });
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: -70 });
  rerender(panel(withSquelch(-70), 2, fns));
  expect(screen.getByRole("button", { name: "Auto" })).toHaveAttribute("aria-pressed", "false");
  // Auto again, then a second click clears the squelch.
  fireEvent.click(screen.getByRole("button", { name: "Auto" }));
  expect(onUpdate).toHaveBeenLastCalledWith(2, { squelchDbfs: -85 });
  rerender(panel(withSquelch(-85), 2, fns));
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

it("phone sheet: the Demodulator block offers the digital modes and starts one", async () => {
  const fake = fakePluginApi([pocsag, aprs], []);
  const store = new PluginStore(fake.api);
  const onUpdate = vi.fn<Fns["onUpdate"]>();
  render(
    <ReceiverPanel
      vfo={vfo(1)}
      loaded
      audio={audio}
      audioSnap={audioSnap}
      autoSquelch={autoSquelch}
      locked={false}
      showDemodulator
      digital={{ device: "dev0", plugins: store }}
      onUpdate={onUpdate}
      onRemove={vi.fn()}
    />,
  );
  store.start();
  await act(() => new Promise<void>((r) => setTimeout(r, 0)));
  const select = screen.getByRole("combobox", { name: "Digital mode" });
  expect(select).toHaveClass("w-full");
  expect(
    within(select)
      .getAllByRole("option")
      .map((o) => o.textContent),
  ).toEqual(["Digital…", "POCSAG", "APRS"]);
  fireEvent.change(select, { target: { value: "aprs" } });
  await act(() => new Promise<void>((r) => setTimeout(r, 0)));
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("switch"));
  fireEvent.click(screen.getByRole("button", { name: "Start APRS" }));
  await act(() => new Promise<void>((r) => setTimeout(r, 0)));
  expect(fake.api.start).toHaveBeenCalled();
  store.stop();
});

it("without the digital prop the Demodulator block has no digital picker", () => {
  render(panel([vfo(1)], 1, makeFns()));
  expect(screen.queryByRole("combobox", { name: "Digital mode" })).toBeNull();
});
