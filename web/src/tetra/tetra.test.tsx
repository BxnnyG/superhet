import { act, render, renderHook, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EventApi } from "../decoders/api";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { DecoderEventStore } from "../decoders/store";
import { describeEvent } from "../decoders/render";
import { enterDigital } from "../digital/actions";
import { DigitalDock } from "../digital/DigitalDock";
import { DIGITAL_MODES, digitalMode, digitalOnVfo } from "../digital/modes";
import { m17Routes } from "../m17/audio";
import { PluginStore } from "../plugins/store";
import { TETRA_ID, fakePluginApi, instance, tetra as tetraManifest } from "../plugins/testing";
import { vfo } from "../shell/fixtures";
import { toasts } from "../ui";
import { TetraNow } from "./TetraNow";
import {
  EMPTY_STATUS,
  cellEncryptionLabel,
  cellLabel,
  channelLabel,
  encryptionLabel,
  formatDuration,
  parseCall,
  parseCell,
  parseStatus,
  targetLabel,
  type TetraStatus,
} from "./model";

// The dock polls the status document; the tests set what it answers.
let polled: { status: TetraStatus; stale: boolean } = { status: EMPTY_STATUS, stale: false };
vi.mock("./useTetraStatus", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./useTetraStatus")>()),
  useTetraStatus: vi.fn(() => polled),
}));

beforeEach(() => {
  localStorage.clear();
  toasts.clear();
  polled = { status: EMPTY_STATUS, stale: false };
});
afterEach(() => vi.useRealTimers());

const CALL = {
  call: "1700000001000-77",
  cid: 77,
  usage: 5,
  dst: 9001,
  src: 2630001,
  encrypted: false,
};

const CELL = {
  mcc: 262,
  mnc: 42,
  cc: 0,
  la: 1,
  dl_hz: 392_012_500,
  encryption: "none",
  encrypted_traffic_seen: false,
  services: ["voice"],
};

const READY = { state: "ready", path: "/usr/bin/tetra-rx" };
const CODEC_READY = { state: "ready", dir: "/opt/tetra-codec" };
const CODEC_MISSING = {
  state: "missing",
  message: "Voice needs the ETSI TETRA codec.",
  searched: ["SUPERHET_TETRA_CODEC_DIR is not set"],
};

const doc = (over: Record<string, unknown> = {}) =>
  parseStatus({
    status: {
      tool: READY,
      codec: CODEC_READY,
      signal: { sync: true, offset_hz: -120 },
      cell: CELL,
      neighbours: [],
      now: [],
      last: null,
      calls: 0,
      ...over,
    },
  });

describe("TETRA model", () => {
  it("reads a call and ignores documents that are none", () => {
    expect(parseCall(CALL)).toMatchObject({
      id: "1700000001000-77",
      cid: 77,
      usage: 5,
      dst: 9001,
      src: 2630001,
      encrypted: false,
      muted: false,
      talker: null,
    });
    // A call missed its set-up: only the traffic channel is known.
    expect(parseCall({ call: "x-u9", usage: 9, encrypted: false })).toMatchObject({ src: null });
    expect(parseCall({ usage: 9 })).toBeNull();
    expect(parseCall(null)).toBeNull();
  });

  it("reads the cell, the status document and tolerates foreign ones", () => {
    expect(parseCell(CELL)).toMatchObject({ mcc: 262, mnc: 42, la: 1, dlHz: 392_012_500 });
    expect(parseCell({ mcc: 262 })).toBeNull();
    const status = doc({
      neighbours: [{ dl_hz: 392_037_500, mcc: 262, mnc: 42, la: 2 }, { junk: 1 }],
      now: [CALL],
      calls: 4,
    });
    expect(status.sync).toBe(true);
    expect(status.offsetHz).toBe(-120);
    expect(status.neighbours).toHaveLength(1);
    expect(status.now[0]?.src).toBe(2630001);
    expect(status.calls).toBe(4);
    expect(status.codec.state).toBe("ready");
    expect(parseStatus({ status: {} })).toEqual(EMPTY_STATUS);
    expect(parseStatus(null)).toEqual(EMPTY_STATUS);
    expect(parseStatus({ status: { codec: CODEC_MISSING } }).codec).toMatchObject({
      state: "missing",
      searched: ["SUPERHET_TETRA_CODEC_DIR is not set"],
    });
  });

  it("names the cell, the parties and the encryption in plain words", () => {
    const cell = parseCell(CELL);
    const air = parseCell({ ...CELL, encryption: "air-interface" });
    const seen = parseCell({ ...CELL, encrypted_traffic_seen: true });
    const call = parseCall(CALL);
    const locked = parseCall({ ...CALL, encrypted: true });
    if (!cell || !air || !seen || !call || !locked) throw new Error("fixture");
    expect(cellLabel(cell)).toBe("262/42 · LA 1 · CC 0 · 392.0125 MHz");
    expect(cellEncryptionLabel(cell)).toBeNull();
    expect(cellEncryptionLabel(air)).toBe("air interface encryption on");
    expect(cellEncryptionLabel(seen)).toBe("encrypted traffic seen");
    expect(targetLabel(call)).toBe("9001");
    expect(channelLabel(call)).toBe("TCH 5");
    expect(encryptionLabel(call)).toBe("clear");
    expect(encryptionLabel(locked)).toBe("encrypted");
    expect(formatDuration(1200)).toBe("1.2 s");
    expect(formatDuration(125_000)).toBe("2 min 05 s");
  });
});

describe("TETRA mode", () => {
  it("is one picker entry: a 25 kHz NFM-preset VFO, no config, no frequency hint", () => {
    const mode = DIGITAL_MODES.find((m) => m.id === "tetra");
    if (!mode) throw new Error("tetra mode");
    expect(mode.pluginId).toBe(TETRA_ID);
    expect(mode.vfo).toMatchObject({ mode: "nfm", bandwidthHz: 25_000, squelchDbfs: null });
    expect(mode.hints).toEqual([]);
    expect(mode.config).toBeUndefined();
    expect(mode.expect).toContain("ETSI");
    expect(mode.expect).toContain("never decrypted");
    expect(
      digitalOnVfo([instance({ id: "a", plugin_id: TETRA_ID, vfo: 1 })], "dev0", 1)[0]?.mode.id,
    ).toBe("tetra");
  });

  it("starts the plugin after the notice is confirmed", async () => {
    const fake = fakePluginApi([tetraManifest], []);
    const store = new PluginStore(fake.api);
    await store.refresh();
    const updateVfo = vi.fn();
    const ctx = { store, device: "dev0", vfo: vfo(1), updateVfo };
    expect(await enterDigital(ctx, digitalMode("tetra"))).toBe("notice");
    expect(await enterDigital(ctx, digitalMode("tetra"), true)).toBe("started");
    expect(fake.api.start).toHaveBeenCalledWith(expect.objectContaining({ pluginId: TETRA_ID }));
    expect(updateVfo).toHaveBeenCalledWith(
      1,
      expect.objectContaining({ mode: "nfm", bandwidthHz: 25_000 }),
    );
  });

  it("routes the decoded voice like M17's", () => {
    const list = [instance({ id: "a", plugin_id: TETRA_ID, vfo: 2, audio_vfo: 1073741900 })];
    expect(m17Routes(list, "dev0")).toEqual({ audioVfos: [1073741900], receivers: [2] });
  });
});

function ev(id: number, kind: string, data: Record<string, unknown>): DecoderEvent {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "dev0",
    vfo: 1,
    freq_hz: 392_012_500,
    mode: "nfm",
    kind,
    source: TETRA_ID,
    value: "",
    meta: { instance: "i-1", data },
  });
  if (!e) throw new Error("fixture");
  return e;
}

describe("TETRA events in the log", () => {
  it("shows a finished call with its parties, channel and length", () => {
    const view = describeEvent(
      ev(1, "tetra.call.end", {
        ...CALL,
        talker: 2630002,
        duration_ms: 12_400,
        voice_ms: 9_000,
        audio_ms: 9_000,
        reason: "release",
      }),
    );
    expect(view.label).toBe("TETRA");
    expect(view.headline).toBe("2630001 → 9001");
    expect(view.tags).toEqual(["TCH 5"]);
    expect(view.detail).toBe("12 s");
    expect(view.fields.find((f) => f.label === "Last talker (SSI)")?.value).toBe("2630002");
  });

  it("flags an encrypted call, and a clear one whose voice was not decoded", () => {
    const locked = describeEvent(
      ev(2, "tetra.call.end", {
        ...CALL,
        encrypted: true,
        muted: true,
        duration_ms: 3000,
        voice_ms: 1200,
        audio_ms: 0,
        reason: "lost",
      }),
    );
    expect(locked.tone).toBe("warn");
    expect(locked.tags).toContain("encrypted");
    expect(locked.detail).toContain("audio withheld");
    const silent = describeEvent(
      ev(3, "tetra.call.end", {
        ...CALL,
        duration_ms: 3000,
        voice_ms: 1200,
        audio_ms: 0,
        reason: "release",
      }),
    );
    expect(silent.detail).toContain("voice not decoded");
  });

  it("shows short messages, status values, the cell and neighbours", () => {
    const sds = describeEvent(
      ev(4, "tetra.sds", { src: 2630001, dst: 9001, text: "Hello team", info: "..." }),
    );
    expect(sds.label).toBe("TETRA SDS");
    expect(sds.headline).toBe("2630001 → 9001");
    expect(sds.detail).toBe("Hello team");
    const status = describeEvent(ev(5, "tetra.status", { src: 2630001, dst: 9001, status: 32770 }));
    expect(status.detail).toBe("status 32770");
    const cell = describeEvent(ev(6, "tetra.cell", CELL));
    expect(cell.headline).toBe("262/42 · LA 1 · CC 0 · 392.0125 MHz");
    expect(cell.detail).toBe("voice");
    const neighbour = describeEvent(
      ev(7, "tetra.neighbour", { dl_hz: 392_037_500, mcc: 262, mnc: 42, la: 2 }),
    );
    expect(neighbour.headline).toBe("392.0375 MHz");
    expect(neighbour.tags).toEqual(["LA 2"]);
  });

  it("falls back to the generic view for a malformed event", () => {
    expect(describeEvent(ev(8, "tetra.call.end", { nonsense: true })).label).toBe("tetra.call.end");
  });
});

describe("Now (TETRA)", () => {
  const live = { started_unix_ms: Date.now() };

  it("shows the cell in sync and the call on the air with its talker", () => {
    render(
      <TetraNow
        status={doc({ now: [{ ...CALL, ...live, talker: 2630002 }], neighbours: [{ dl_hz: 1 }] })}
        stale={false}
        running
      />,
    );
    const box = screen.getByTestId("tetra-now");
    expect(box).toHaveTextContent("Cell in sync");
    expect(screen.getByTestId("tetra-cell")).toHaveTextContent("262/42 · LA 1 · CC 0");
    expect(box).toHaveTextContent("1 neighbour cell");
    expect(box).toHaveTextContent("-120 Hz");
    const call = screen.getByTestId("tetra-now-call");
    expect(screen.getByTestId("tetra-now-src")).toHaveTextContent("2630002");
    expect(call).toHaveTextContent("9001");
    expect(call).toHaveTextContent("TCH 5");
    expect(screen.queryByTestId("tetra-needs-codec")).toBeNull();
  });

  it("flags an encrypted call as muted and the cell's encryption", () => {
    render(
      <TetraNow
        status={doc({
          cell: { ...CELL, encryption: "air-interface" },
          now: [{ ...CALL, ...live, encrypted: true, muted: true }],
        })}
        stale={false}
        running
      />,
    );
    expect(screen.getByTestId("tetra-now")).toHaveTextContent("air interface encryption on");
    expect(screen.getByTestId("tetra-now-call")).toHaveTextContent("encrypted · muted");
  });

  it("searches while there is no sync and is idle between calls", () => {
    render(
      <TetraNow
        status={doc({
          cell: null,
          signal: { sync: false },
          last: { ...CALL, duration_ms: 5000 },
        })}
        stale={false}
        running
      />,
    );
    const box = screen.getByTestId("tetra-now");
    expect(box).toHaveTextContent("Searching for a cell");
    expect(box).toHaveTextContent("Last: 2630001 → 9001 · 5.0 s");
    expect(screen.queryByTestId("tetra-cell")).toBeNull();
  });

  it("does not claim a call when the status is stale", () => {
    render(<TetraNow status={doc({ now: [{ ...CALL, ...live }] })} stale running />);
    expect(screen.getByTestId("tetra-now")).toHaveTextContent("Status unavailable");
    expect(screen.queryByTestId("tetra-now-call")).toBeNull();
  });

  it("says voice needs the ETSI codec while the rest keeps showing", () => {
    render(<TetraNow status={doc({ codec: CODEC_MISSING })} stale={false} running />);
    const box = screen.getByTestId("tetra-needs-codec");
    expect(box).toHaveTextContent("Voice needs the ETSI codec");
    expect(box).toHaveTextContent("SUPERHET_TETRA_CODEC_DIR");
    expect(box).toHaveTextContent("docs/plugins-tetra.md");
    expect(box).toHaveTextContent("never downloaded");
    expect(box).toHaveTextContent("Looked for: SUPERHET_TETRA_CODEC_DIR is not set");
    // The signalling is still there.
    expect(screen.getByTestId("tetra-now")).toHaveTextContent("Cell in sync");
    expect(screen.getByTestId("tetra-cell")).toBeInTheDocument();
  });

  it("shows a stopped codec as an error and a deliberately off one not at all", () => {
    const { unmount } = render(
      <TetraNow
        status={doc({ codec: { state: "failed", detail: "the codec programs ended" } })}
        stale={false}
        running
      />,
    );
    expect(screen.getByTestId("tetra-needs-codec")).toHaveTextContent("The ETSI codec stopped");
    expect(screen.getByTestId("tetra-needs-codec")).toHaveTextContent("the codec programs ended");
    unmount();
    render(<TetraNow status={doc({ codec: { state: "disabled" } })} stale={false} running />);
    expect(screen.queryByTestId("tetra-needs-codec")).toBeNull();
  });

  it("asks for tetra-rx instead of showing a channel when it is not installed", () => {
    const missing = parseStatus({
      status: {
        tool: { state: "missing", message: "tetra-rx is not installed.", searched: ["PATH"] },
      },
    });
    render(<TetraNow status={missing} stale={false} running />);
    const box = screen.getByTestId("tetra-needs-tool");
    expect(box).toHaveTextContent("tetra-rx not found");
    expect(box).toHaveTextContent("SUPERHET_TETRA_RX_PATH");
    expect(screen.queryByTestId("tetra-now")).toBeNull();
  });

  it("shows a decoder that cannot be started as an error", () => {
    const failed = parseStatus({
      status: { tool: { state: "failed", message: "cannot start: permission denied" } },
    });
    render(<TetraNow status={failed} stale={false} running />);
    expect(screen.getByTestId("tetra-tool-failed")).toHaveTextContent("permission denied");
  });
});

describe("TETRA in the output dock", () => {
  const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

  async function setup(events: DecoderEvent[]) {
    const fake = fakePluginApi(
      [tetraManifest],
      [instance({ id: "i-1", plugin_id: TETRA_ID, vfo: 1, config: null })],
    );
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
        vfos={[vfo(1, { freqHz: 392_012_500 })]}
        selectedId={1}
        createEvents={() => events$}
      />,
    );
    store.start();
    await flush();
    return store;
  }

  it("lists finished calls, messages and status values; the start and cell events stay out", async () => {
    polled = {
      status: doc({ now: [{ ...CALL, src: 999, started_unix_ms: Date.now() }], calls: 2 }),
      stale: false,
    };
    const store = await setup([
      ev(9, "tetra.cell", CELL),
      ev(8, "tetra.call.start", { ...CALL, src: 999 }),
      ev(7, "tetra.sds", { src: 2630001, dst: 9001, text: "Hello team", info: "x" }),
      ev(6, "tetra.status", { src: 2630001, dst: 9001, status: 32770 }),
      ev(5, "tetra.call.end", {
        ...CALL,
        duration_ms: 8000,
        voice_ms: 7000,
        audio_ms: 7000,
        reason: "release",
      }),
    ]);
    const dock = screen.getByTestId("digital-dock");
    expect(dock).toHaveAttribute("aria-label", "TETRA decoder output");
    const rows = within(dock).getAllByRole("row");
    // Header + call end + message + status.
    expect(rows).toHaveLength(4);
    expect(within(dock).getByTestId("tetra-now-src")).toHaveTextContent("999");
    expect(within(dock).getAllByText("Hello team").length).toBeGreaterThan(0);
    store.stop();
  });

  it("tells that voice needs the ETSI codec in the header and the panel, and that tetra-rx is there", async () => {
    polled = { status: doc({ codec: CODEC_MISSING }), stale: false };
    const store = await setup([]);
    const dock = screen.getByTestId("digital-dock");
    expect(within(dock).getByText("voice needs the ETSI codec")).toBeInTheDocument();
    expect(within(dock).getByTestId("tetra-needs-codec")).toHaveTextContent(
      "Voice needs the ETSI codec",
    );
    expect(within(dock).queryByText("needs tetra-rx")).toBeNull();
    expect(screen.getByTestId("dock-empty")).toHaveTextContent("Listening");
    store.stop();
  });

  it("says tetra-rx is needed when it is missing", async () => {
    polled = {
      status: parseStatus({
        status: { tool: { state: "missing", message: "tetra-rx is not installed." } },
      }),
      stale: false,
    };
    const store = await setup([]);
    const dock = screen.getByTestId("digital-dock");
    expect(within(dock).getByText("needs tetra-rx")).toBeInTheDocument();
    expect(screen.getByTestId("dock-empty")).toHaveTextContent("Waiting for tetra-rx");
    store.stop();
  });
});

describe("useTetraStatus", () => {
  it("polls the status of the instance and goes stale when the poll fails", async () => {
    vi.useFakeTimers();
    const { useTetraStatus: real } =
      await vi.importActual<typeof import("./useTetraStatus")>("./useTetraStatus");
    const fetchStatus = vi
      .fn<(id: string, signal: AbortSignal) => Promise<TetraStatus>>()
      .mockResolvedValueOnce(doc({ now: [CALL], calls: 1 }))
      .mockRejectedValueOnce(new Error("down"));
    const { result } = renderHook(() => real("i-1", fetchStatus));
    expect(result.current.status).toEqual(EMPTY_STATUS);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(result.current.status.now[0]?.src).toBe(2630001);
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
