import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { AudioController, AudioSnapshot } from "../audio/controller";
import type { RdsState } from "../audio/rds";
import type { ToneState } from "../audio/tones";
import type { Vfo } from "../vfo/api";
import { LiveCards } from "./LiveCards";

const vfo = (over: Partial<Vfo> = {}): Vfo => ({
  id: 1,
  freqHz: 99_200_000,
  mode: "wfm",
  bandwidthHz: 180_000,
  squelchDbfs: null,
  deemphasis: "50us",
  offsetHz: 0,
  status: "active",
  ...over,
});

const rds = (over: Partial<RdsState> = {}): RdsState => ({
  synced: true,
  pi: "D220",
  pty: 10,
  tp: true,
  ta: false,
  ms: true,
  ps: "RADIO X ",
  rt: "Now playing: Song",
  bler: null,
  quality: null,
  ...over,
});

const snap = (r: Record<number, RdsState> = {}, t: Record<number, ToneState> = {}) =>
  ({ rds: r, tones: t }) as unknown as AudioSnapshot;

function setup(vfos: Vfo[], s: AudioSnapshot, selectedId: number | null = vfos[0]?.id ?? null) {
  const audio = { clearDigits: vi.fn() } as unknown as AudioController;
  const onUpdate = vi.fn();
  render(
    <LiveCards
      vfos={vfos}
      selectedId={selectedId}
      audio={audio}
      audioSnap={s}
      onUpdate={onUpdate}
    />,
  );
  return { audio, onUpdate };
}

it("shows the station name big with RadioText, PI, PTY and the sync state", () => {
  setup([vfo()], snap({ 1: rds() }));
  expect(screen.getByTestId("rds-ps")).toHaveTextContent("RADIO X");
  expect(screen.getByTestId("rds-rt")).toHaveTextContent("Now playing: Song");
  expect(screen.getByText("PI D220")).toBeInTheDocument();
  expect(screen.getByText("TP")).toBeInTheDocument();
  expect(screen.getByText("RDS sync")).toBeInTheDocument();
});

it("grades the reception by the block error rate", () => {
  const { unmount } = render(
    <LiveCards
      vfos={[vfo()]}
      selectedId={1}
      audio={{ clearDigits: vi.fn() } as unknown as AudioController}
      audioSnap={snap({ 1: rds({ bler: 4, quality: 96 }) })}
      onUpdate={vi.fn()}
    />,
  );
  expect(screen.getByTestId("rds-quality")).toHaveAttribute("data-level", "good");
  expect(screen.getByText(/4 % BLER/)).toBeInTheDocument();
  expect(screen.getByRole("meter", { name: "RDS reception quality" })).toHaveAttribute(
    "aria-valuenow",
    "96",
  );
  expect(screen.queryByText(/too weak/)).not.toBeInTheDocument();
  unmount();
});

it("warns that the signal is too weak for text when blocks fail a lot", () => {
  setup([vfo()], snap({ 1: rds({ bler: 62, quality: 38, ps: null, rt: null }) }));
  expect(screen.getByTestId("rds-quality")).toHaveAttribute("data-level", "poor");
  expect(screen.getByText(/too weak for the station name/)).toBeInTheDocument();
});

it("does not call a decoded station too weak", () => {
  setup([vfo()], snap({ 1: rds({ bler: 62, quality: 38 }) }));
  expect(screen.queryByText(/too weak/)).not.toBeInTheDocument();
});

it("says explicitly when there is no RDS sync", () => {
  setup([vfo()], snap());
  expect(screen.getByText("no RDS sync")).toBeInTheDocument();
  expect(screen.getByText(/Waiting for RDS/)).toBeInTheDocument();
});

it("marks lost sync while keeping the last data", () => {
  setup([vfo()], snap({ 1: rds({ synced: false }) }));
  expect(screen.getByText("signal lost")).toBeInTheDocument();
  expect(screen.getByTestId("rds-ps")).toHaveTextContent("RADIO X");
});

it("shows CTCSS and the last DTMF sequence of an NFM receiver with its controls", () => {
  const nfm = vfo({ id: 3, mode: "nfm", freqHz: 446_006_250, tones: true, toneSquelchHz: null });
  const { audio, onUpdate } = setup(
    [nfm],
    snap({}, { 3: { ctcssHz: 88.5, digits: "12*A", count: 4 } }),
  );
  expect(screen.getByTestId("ctcss")).toHaveTextContent("88.5 Hz");
  expect(screen.getByTestId("dtmf-digits")).toHaveTextContent("12*A");
  fireEvent.click(screen.getByRole("button", { name: "Use" }));
  expect(onUpdate).toHaveBeenCalledWith(3, { toneSquelchHz: 88.5 });
  fireEvent.click(screen.getByRole("button", { name: "Clear" }));
  expect(
    (audio as unknown as { clearDigits: ReturnType<typeof vi.fn> }).clearDigits,
  ).toHaveBeenCalledWith(3);
  fireEvent.click(screen.getByRole("switch", { name: "Detect" }));
  expect(onUpdate).toHaveBeenCalledWith(3, { tones: false, toneSquelchHz: null });
});

it("explains a disabled tone detector", () => {
  setup([vfo({ id: 2, mode: "nfm", tones: false })], snap());
  expect(screen.getByText(/detection is off/)).toBeInTheDocument();
});

it("lists one card per decoding VFO, the selected one first, and skips other modes", () => {
  setup(
    [vfo({ id: 1, mode: "usb" }), vfo({ id: 2 }), vfo({ id: 3, mode: "nfm", tones: true })],
    snap(),
    3,
  );
  const cards = screen.getAllByRole("region");
  expect(cards).toHaveLength(2);
  expect(within(cards[0] as HTMLElement).getByText(/VFO 3/)).toBeInTheDocument();
  expect(within(cards[1] as HTMLElement).getByText(/VFO 2/)).toBeInTheDocument();
});

it("shows no RDS card for a WFM receiver outside the FM broadcast band or too narrow", () => {
  setup([vfo({ id: 1, freqHz: 433_920_000 })], snap());
  expect(screen.queryByTestId("rds-ps")).not.toBeInTheDocument();
  expect(screen.getByText(/RDS needs a WFM receiver on the FM broadcast band/)).toBeInTheDocument();
});

it("needs enough bandwidth for the RDS subcarrier", () => {
  const { unmount } = render(
    <LiveCards
      vfos={[vfo({ bandwidthHz: 100_000 })]}
      selectedId={1}
      audio={{ clearDigits: vi.fn() } as unknown as AudioController}
      audioSnap={snap()}
      onUpdate={vi.fn()}
    />,
  );
  expect(screen.queryByTestId("rds-ps")).not.toBeInTheDocument();
  unmount();
  setup([vfo({ bandwidthHz: 120_000 })], snap());
  expect(screen.getByTestId("rds-ps")).toBeInTheDocument();
});

it("explains when no receiver has a decoder", () => {
  setup([vfo({ mode: "usb" })], snap());
  expect(screen.getByText("No decoders for this mode")).toBeInTheDocument();
});
