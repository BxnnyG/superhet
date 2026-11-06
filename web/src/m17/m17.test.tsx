import { act, render, renderHook, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EventApi } from "../decoders/api";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { DecoderEventStore } from "../decoders/store";
import { describeEvent } from "../decoders/render";
import { DigitalDock } from "../digital/DigitalDock";
import { PluginStore } from "../plugins/store";
import { M17_ID, fakePluginApi, instance, m17 as m17Manifest } from "../plugins/testing";
import { vfo } from "../shell/fixtures";
import { toasts } from "../ui";
import { m17Routes } from "./audio";
import {
  EMPTY_STATUS,
  encryptionLabel,
  formatDuration,
  parseCall,
  parseStatus,
  type M17Status,
} from "./model";
import { NowTalking } from "./NowTalking";

// The dock polls the status document; the tests set what it answers.
let polled: { status: M17Status; stale: boolean } = { status: EMPTY_STATUS, stale: false };
vi.mock("./useM17Status", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./useM17Status")>()),
  useM17Status: vi.fn(() => polled),
}));

beforeEach(() => {
  localStorage.clear();
  toasts.clear();
  polled = { status: EMPTY_STATUS, stale: false };
});
afterEach(() => vi.useRealTimers());

const CALL = {
  call: "1700000000000",
  src: "DL1ABC",
  dst: "DB0XYZ",
  can: 3,
  mode: "voice",
  encrypted: false,
  encryption: "none",
};

describe("M17 model", () => {
  it("reads a call and ignores documents that are none", () => {
    expect(parseCall(CALL)).toMatchObject({
      id: "1700000000000",
      src: "DL1ABC",
      dst: "DB0XYZ",
      can: 3,
      encrypted: false,
      muted: false,
      position: null,
    });
    expect(parseCall({ src: "DL1ABC" })).toBeNull();
    expect(parseCall(null)).toBeNull();
    // A decoder build that does not report the encryption bits.
    expect(parseCall({ ...CALL, encrypted: undefined })?.encrypted).toBeNull();
  });

  it("reads a position and refuses impossible ones", () => {
    const call = parseCall({
      ...CALL,
      position: { lat: 48.1, lon: -9.25, altitude_m: 600, station: "handheld" },
    });
    expect(call?.position).toMatchObject({ lat: 48.1, lon: -9.25, altitudeM: 600 });
    expect(parseCall({ ...CALL, position: { lat: 95, lon: 0 } })?.position).toBeNull();
  });

  it("reads the status document of the instance reply", () => {
    const status = parseStatus({
      status: { now: { ...CALL, started_unix_ms: 1_700_000_000_000 }, last: null, calls: 4 },
    });
    expect(status.now?.startedMs).toBe(1_700_000_000_000);
    expect(status.calls).toBe(4);
    expect(parseStatus({ status: null })).toEqual(EMPTY_STATUS);
    expect(parseStatus("nonsense")).toEqual(EMPTY_STATUS);
  });

  it("formats durations and encryption", () => {
    expect(formatDuration(1234)).toBe("1.2 s");
    expect(formatDuration(34_000)).toBe("34 s");
    expect(formatDuration(125_000)).toBe("2 min 05 s");
    const aes = parseCall({ ...CALL, encrypted: true, encryption: "aes" });
    expect(aes && encryptionLabel(aes)).toBe("AES encrypted");
    const clear = parseCall(CALL);
    expect(clear && encryptionLabel(clear)).toBe("clear");
  });

  it("finds the audio streams and receivers of the M17 instances on a device", () => {
    const list = [
      instance({ id: "a", plugin_id: M17_ID, vfo: 2, audio_vfo: 1073741900 }),
      instance({ id: "b", plugin_id: M17_ID, vfo: 3, device: "dev1", audio_vfo: 1073741901 }),
      instance({ id: "c", vfo: 1 }),
    ];
    expect(m17Routes(list, "dev0")).toEqual({ audioVfos: [1073741900], receivers: [2] });
  });
});

function ev(
  id: number,
  kind: string,
  data: Record<string, unknown>,
  instanceId = "i-1",
): DecoderEvent {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "dev0",
    vfo: 1,
    freq_hz: 144_910_000,
    mode: "nfm",
    kind,
    source: M17_ID,
    value: "",
    meta: { instance: instanceId, data },
  });
  if (!e) throw new Error("fixture");
  return e;
}

describe("M17 events in the log", () => {
  it("shows a finished call with its length", () => {
    const view = describeEvent(
      ev(1, "m17.call.end", { ...CALL, duration_ms: 12_400, audio_ms: 11_200, reason: "eos" }),
    );
    expect(view.label).toBe("M17");
    expect(view.headline).toBe("DL1ABC → DB0XYZ");
    expect(view.tags).toEqual(["CAN 3"]);
    expect(view.detail).toBe("12 s");
    expect(view.fields.find((f) => f.label === "Encryption")?.value).toBe("clear");
  });

  it("marks an encrypted call and says its audio was withheld", () => {
    const view = describeEvent(
      ev(2, "m17.call.end", {
        ...CALL,
        encrypted: true,
        encryption: "aes",
        muted: true,
        duration_ms: 3000,
        audio_ms: 0,
        reason: "eos",
      }),
    );
    expect(view.tone).toBe("warn");
    expect(view.tags).toContain("AES encrypted");
    expect(view.detail).toContain("audio withheld");
  });

  it("shows text and position, with a map link", () => {
    const view = describeEvent(
      ev(3, "m17.call.start", {
        ...CALL,
        text: "Hello",
        position: { lat: 48.12345, lon: 11.5, station: "mobile" },
      }),
    );
    expect(view.detail).toContain("“Hello”");
    expect(view.detail).toContain("48.12345, 11.50000");
    expect(view.link?.href).toContain("mlat=48.12345");
    expect(view.title).toBe("M17 call started");
  });

  it("falls back to the generic view for a malformed event", () => {
    const view = describeEvent(ev(4, "m17.call.end", { nonsense: true }));
    expect(view.label).toBe("m17.call.end");
  });
});

describe("Now talking", () => {
  const status = (now: Record<string, unknown> | null, last = null): M17Status => ({
    now: now === null ? null : parseCall({ ...CALL, started_unix_ms: Date.now(), ...now }),
    last,
    calls: 1,
  });

  it("shows the station on the air with destination, CAN and encryption", () => {
    render(
      <NowTalking
        status={status({ encrypted: true, encryption: "aes", muted: true })}
        stale={false}
        running
      />,
    );
    const box = screen.getByTestId("m17-now");
    expect(within(box).getByTestId("m17-now-src")).toHaveTextContent("DL1ABC");
    expect(box).toHaveTextContent("DB0XYZ");
    expect(box).toHaveTextContent("CAN 3");
    expect(box).toHaveTextContent("AES encrypted · muted");
    expect(box).toHaveTextContent("On air");
  });

  it("is idle between calls and mentions the last one", () => {
    const last = parseCall({ ...CALL, duration_ms: 5000 });
    render(<NowTalking status={{ now: null, last, calls: 2 }} stale={false} running />);
    const box = screen.getByTestId("m17-now");
    expect(box).toHaveTextContent("Channel idle");
    expect(box).toHaveTextContent("Last: DL1ABC → DB0XYZ · 5.0 s");
  });

  it("does not claim a call when the status is stale", () => {
    render(<NowTalking status={status({})} stale running />);
    const box = screen.getByTestId("m17-now");
    expect(box).toHaveTextContent("Status unavailable");
    expect(box).not.toHaveTextContent("On air");
  });

  it("waits for the decoder while it is not running", () => {
    render(<NowTalking status={EMPTY_STATUS} stale={false} running={false} />);
    expect(screen.getByTestId("m17-now")).toHaveTextContent("Waiting for the decoder");
  });
});

describe("M17 in the output dock", () => {
  const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

  async function setup(events: DecoderEvent[]) {
    const fake = fakePluginApi([m17Manifest], [instance({ id: "i-1", plugin_id: M17_ID, vfo: 1 })]);
    const store = new PluginStore(fake.api);
    const eventApi = {
      list: vi.fn<EventApi["list"]>(() => Promise.resolve({ events, next: null })),
      clear: vi.fn<EventApi["clear"]>(() => Promise.resolve(0)),
    } satisfies EventApi;
    const events$ = new DecoderEventStore({
      device: "dev0",
      api: eventApi,
      createSocket: () => ({ close() {} }) as unknown as WebSocket,
    });
    render(
      <DigitalDock
        plugins={store}
        device="dev0"
        vfos={[vfo(1, { freqHz: 144_910_000 })]}
        selectedId={1}
        createEvents={() => events$}
      />,
    );
    store.start();
    await flush();
    return store;
  }

  it("lists finished calls only and shows the call on the air above them", async () => {
    polled = {
      status: {
        now: parseCall({ ...CALL, src: "DL9LIVE", started_unix_ms: Date.now() }),
        last: null,
        calls: 2,
      },
      stale: false,
    };
    const store = await setup([
      ev(5, "m17.call.start", { ...CALL, src: "DL9LIVE", call: "5" }),
      ev(4, "m17.call.end", { ...CALL, duration_ms: 8000, audio_ms: 7000, reason: "eos" }),
    ]);
    const dock = screen.getByTestId("digital-dock");
    // Header + the one finished call; the start event is not a row of its own.
    const rows = within(dock).getAllByRole("row");
    expect(rows).toHaveLength(2);
    expect(rows[1]).toHaveTextContent("DL1ABC → DB0XYZ");
    expect(rows[1]).toHaveTextContent("8.0 s");
    expect(within(dock).getByText("Call")).toBeInTheDocument();
    // The call on the air: banner and a badge in the header.
    expect(within(dock).getByTestId("m17-now-src")).toHaveTextContent("DL9LIVE");
    expect(within(dock).getAllByText("DL9LIVE").length).toBeGreaterThanOrEqual(2);
    store.stop();
  });

  it("says so when no call was decoded yet", async () => {
    const store = await setup([]);
    expect(screen.getByTestId("dock-empty")).toHaveTextContent("No calls yet on 144.910 MHz");
    expect(screen.getByTestId("m17-now")).toHaveTextContent("Channel idle");
    store.stop();
  });
});

describe("useM17Status", () => {
  it("polls the status of the instance and goes stale when the poll fails", async () => {
    vi.useFakeTimers();
    const { useM17Status: real } =
      await vi.importActual<typeof import("./useM17Status")>("./useM17Status");
    const doc = { now: parseCall(CALL), last: null, calls: 1 } satisfies M17Status;
    const fetchStatus = vi
      .fn<(id: string, signal: AbortSignal) => Promise<M17Status>>()
      .mockResolvedValueOnce(doc)
      .mockRejectedValueOnce(new Error("down"));
    const { result } = renderHook(() => real("i-1", fetchStatus));
    expect(result.current.status).toEqual(EMPTY_STATUS);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(result.current.status.now?.src).toBe("DL1ABC");
    expect(result.current.stale).toBe(false);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1100);
    });
    expect(fetchStatus).toHaveBeenCalledTimes(2);
    expect(result.current.stale).toBe(true);
    // Without an instance nothing is polled.
    const idle = renderHook(() => real(null, fetchStatus));
    expect(idle.result.current).toEqual({ status: EMPTY_STATUS, stale: false });
    expect(fetchStatus).toHaveBeenCalledTimes(2);
  });
});
