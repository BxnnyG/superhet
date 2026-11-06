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
import { DSD_ID, dsd as dsdManifest, fakePluginApi, instance } from "../plugins/testing";
import { vfo } from "../shell/fixtures";
import { toasts } from "../ui";
import { DsdNow } from "./DsdNow";
import {
  EMPTY_STATUS,
  channelLabel,
  encryptionLabel,
  formatDuration,
  parseCall,
  parseStatus,
  targetLabel,
  type DsdStatus,
} from "./model";

// The dock polls the status document; the tests set what it answers.
let polled: { status: DsdStatus; stale: boolean } = { status: EMPTY_STATUS, stale: false };
vi.mock("./useDsdStatus", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./useDsdStatus")>()),
  useDsdStatus: vi.fn(() => polled),
}));

beforeEach(() => {
  localStorage.clear();
  toasts.clear();
  polled = { status: EMPTY_STATUS, stale: false };
});
afterEach(() => vi.useRealTimers());

const CALL = {
  call: "1700000001900-s1",
  mode: "dmr",
  slot: 1,
  src: 3101234,
  dst: 91,
  group: true,
  cc: 1,
  encrypted: false,
};

const READY = { state: "ready", path: "/opt/dsd-fme" };
const MISSING = {
  state: "missing",
  message: "dsd-fme is not installed.",
  searched: ["PATH (empty)", "/data/plugins/x/bin/dsd-fme"],
};

describe("DSD model", () => {
  it("reads a call and ignores documents that are none", () => {
    expect(parseCall(CALL)).toMatchObject({
      id: "1700000001900-s1",
      mode: "dmr",
      slot: 1,
      src: 3101234,
      dst: 91,
      group: true,
      cc: 1,
      encrypted: false,
      muted: false,
    });
    // Link control may name only one side.
    expect(parseCall({ mode: "nxdn", dst: 5, encrypted: false })).toMatchObject({ src: null });
    expect(parseCall({ mode: "dmr" })).toBeNull();
    expect(parseCall(null)).toBeNull();
  });

  it("reads the status document with the tool state and several calls", () => {
    const status = parseStatus({
      status: {
        tool: READY,
        now: [CALL, { ...CALL, slot: 2, src: 77, group: false, dst: 4000123 }, { junk: 1 }],
        last: { ...CALL, duration_ms: 5000 },
        calls: 3,
      },
    });
    expect(status.tool).toMatchObject({ state: "ready", path: "/opt/dsd-fme" });
    expect(status.now.map((c) => c.slot)).toEqual([1, 2]);
    expect(status.last?.durationMs).toBe(5000);
    expect(status.calls).toBe(3);
    // Documents of an older or foreign plugin do not break the panel.
    expect(parseStatus({ status: {} })).toEqual(EMPTY_STATUS);
    expect(parseStatus(null)).toEqual(EMPTY_STATUS);
    expect(parseStatus({ status: { tool: MISSING } }).tool).toMatchObject({
      state: "missing",
      searched: ["PATH (empty)", "/data/plugins/x/bin/dsd-fme"],
    });
  });

  it("names targets, channels and encryption in plain words", () => {
    const group = parseCall(CALL);
    const priv = parseCall({ ...CALL, group: false, dst: 4000123 });
    const nxdn = parseCall({
      mode: "nxdn",
      src: 1,
      dst: 2,
      ran: 4,
      encrypted: true,
      algorithm: "0x21",
    });
    if (!group || !priv || !nxdn) throw new Error("fixture");
    expect(targetLabel(group)).toBe("TG 91");
    expect(targetLabel(priv)).toBe("ID 4000123");
    expect(channelLabel(group)).toBe("slot 1 · CC 1");
    expect(channelLabel(nxdn)).toBe("RAN 4");
    expect(encryptionLabel(group)).toBe("clear");
    expect(encryptionLabel(nxdn)).toBe("encrypted (0x21)");
    expect(formatDuration(1200)).toBe("1.2 s");
    expect(formatDuration(125_000)).toBe("2 min 05 s");
  });
});

describe("DMR / dPMR / NXDN modes", () => {
  it("are three picker entries of one plugin, told apart by the protocol config", () => {
    const dsd = DIGITAL_MODES.filter((m) => m.pluginId === DSD_ID);
    expect(dsd.map((m) => m.id)).toEqual(["dmr", "dpmr", "nxdn"]);
    expect(dsd.map((m) => m.config)).toEqual([
      { protocol: "dmr" },
      { protocol: "dpmr" },
      { protocol: "nxdn" },
    ]);
    for (const m of dsd) {
      expect(m.vfo).toMatchObject({ mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null });
      expect(m.hints).toEqual([]);
      expect(m.expect).toContain("dsd-fme");
    }
  });

  it("find the mode of an instance by its config", () => {
    const here = (config: unknown) =>
      digitalOnVfo([instance({ id: "a", plugin_id: DSD_ID, vfo: 1, config })], "dev0", 1)[0]?.mode
        .id;
    expect(here({ protocol: "nxdn" })).toBe("nxdn");
    expect(here({ protocol: "dpmr", audio: false })).toBe("dpmr");
    expect(here({ protocol: "dmr" })).toBe("dmr");
    // No protocol (auto-detection, started by hand): the first of the plugin's modes.
    expect(here(null)).toBe("dmr");
  });

  it("start the plugin with their protocol and switch between each other", async () => {
    const fake = fakePluginApi([dsdManifest], []);
    const store = new PluginStore(fake.api);
    await store.refresh();
    const updateVfo = vi.fn();
    const ctx = { store, device: "dev0", vfo: vfo(1), updateVfo };
    // The notice has to be confirmed first.
    expect(await enterDigital(ctx, digitalMode("nxdn"))).toBe("notice");
    expect(await enterDigital(ctx, digitalMode("nxdn"), true)).toBe("started");
    expect(fake.api.start).toHaveBeenCalledWith(
      expect.objectContaining({ pluginId: DSD_ID, config: { protocol: "nxdn" } }),
    );
    expect(updateVfo).toHaveBeenCalledWith(1, expect.objectContaining({ mode: "nfm" }));
    // Same mode again: already running. Another mode of the plugin replaces the instance.
    await store.refreshInstances();
    expect(await enterDigital(ctx, digitalMode("nxdn"), true)).toBe("running");
    expect(await enterDigital(ctx, digitalMode("dmr"), true)).toBe("started");
    expect(fake.api.stop).toHaveBeenCalledTimes(1);
    expect(fake.api.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ config: { protocol: "dmr" } }),
    );
  });

  it("route their decoded voice like M17's", () => {
    const list = [
      instance({ id: "a", plugin_id: DSD_ID, vfo: 2, audio_vfo: 1073741900 }),
      instance({ id: "b", plugin_id: DSD_ID, vfo: 3, device: "dev1", audio_vfo: 1073741901 }),
    ];
    expect(m17Routes(list, "dev0")).toEqual({ audioVfos: [1073741900], receivers: [2] });
  });
});

function ev(id: number, kind: string, data: Record<string, unknown>): DecoderEvent {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "dev0",
    vfo: 1,
    freq_hz: 438_000_000,
    mode: "nfm",
    kind,
    source: DSD_ID,
    value: "",
    meta: { instance: "i-1", data },
  });
  if (!e) throw new Error("fixture");
  return e;
}

describe("DSD events in the log", () => {
  it("shows a finished DMR call with slot, colour code and length", () => {
    const view = describeEvent(
      ev(1, "dsd.call.end", { ...CALL, duration_ms: 12_400, audio_ms: 11_200, reason: "eos" }),
    );
    expect(view.label).toBe("DMR");
    expect(view.headline).toBe("3101234 → TG 91");
    expect(view.tags).toEqual(["slot 1 · CC 1"]);
    expect(view.detail).toBe("12 s");
    expect(view.fields.find((f) => f.label === "Encryption")?.value).toBe("clear");
    expect(view.fields.find((f) => f.label === "Call type")?.value).toBe("group");
  });

  it("marks an encrypted call and says its audio was withheld", () => {
    const view = describeEvent(
      ev(2, "dsd.call.end", {
        ...CALL,
        encrypted: true,
        algorithm: "0x21",
        muted: false,
        duration_ms: 3000,
        audio_ms: 0,
        reason: "eos",
      }),
    );
    expect(view.tone).toBe("warn");
    expect(view.tags).toContain("encrypted (0x21)");
  });

  it("falls back to the generic view for a malformed event", () => {
    expect(describeEvent(ev(3, "dsd.call.end", { nonsense: true })).label).toBe("dsd.call.end");
  });
});

describe("Now talking (DSD)", () => {
  const status = (now: Record<string, unknown>[], last: Record<string, unknown> | null = null) =>
    parseStatus({ status: { tool: READY, now, last, calls: 1 } });
  const live = { started_unix_ms: Date.now() };

  it("shows both DMR slots when both are on the air", () => {
    render(
      <DsdNow
        status={status([
          { ...CALL, ...live },
          { ...CALL, ...live, slot: 2, src: 77, group: false, dst: 4000123, call: "2" },
        ])}
        stale={false}
        running
      />,
    );
    const calls = screen.getAllByTestId("dsd-now-call");
    expect(calls).toHaveLength(2);
    expect(calls[0]).toHaveTextContent("3101234");
    expect(calls[0]).toHaveTextContent("TG 91");
    expect(calls[0]).toHaveTextContent("slot 1 · CC 1");
    expect(calls[1]).toHaveTextContent("ID 4000123");
    expect(calls[1]).toHaveTextContent("slot 2");
  });

  it("flags an encrypted call as muted", () => {
    render(
      <DsdNow
        status={status([{ ...CALL, ...live, encrypted: true, algorithm: "0x21", muted: true }])}
        stale={false}
        running
      />,
    );
    expect(screen.getByTestId("dsd-now")).toHaveTextContent("encrypted (0x21) · muted");
  });

  it("is idle between calls and mentions the last one", () => {
    render(<DsdNow status={status([], { ...CALL, duration_ms: 5000 })} stale={false} running />);
    const box = screen.getByTestId("dsd-now");
    expect(box).toHaveTextContent("Channel idle");
    expect(box).toHaveTextContent("Last: 3101234 → TG 91 · 5.0 s");
  });

  it("does not claim a call when the status is stale", () => {
    render(<DsdNow status={status([{ ...CALL, ...live }])} stale running />);
    expect(screen.getByTestId("dsd-now")).toHaveTextContent("Status unavailable");
    expect(screen.queryByTestId("dsd-now-call")).toBeNull();
  });

  it("asks for dsd-fme instead of showing a channel when it is not installed", () => {
    const missing = parseStatus({ status: { tool: MISSING, now: [], last: null, calls: 0 } });
    render(<DsdNow status={missing} stale={false} running />);
    const box = screen.getByTestId("dsd-needs-tool");
    expect(box).toHaveTextContent("Needs dsd-fme");
    expect(box).toHaveTextContent("dsd-fme is not installed.");
    expect(box).toHaveTextContent("SUPERHET_DSD_FME_PATH");
    expect(box).toHaveTextContent("docs/plugins-dsd.md");
    expect(box).toHaveTextContent("Looked for: PATH (empty); /data/plugins/x/bin/dsd-fme");
    expect(box).toHaveTextContent("Nothing is downloaded automatically");
    expect(screen.queryByTestId("dsd-now")).toBeNull();
  });

  it("shows a tool that cannot be started as an error", () => {
    const failed = parseStatus({
      status: { tool: { state: "failed", message: "cannot start: permission denied" } },
    });
    render(<DsdNow status={failed} stale={false} running />);
    expect(screen.getByTestId("dsd-tool-failed")).toHaveTextContent("permission denied");
  });
});

describe("DSD in the output dock", () => {
  const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

  async function setup(events: DecoderEvent[], config: unknown = { protocol: "dmr" }) {
    const fake = fakePluginApi(
      [dsdManifest],
      [instance({ id: "i-1", plugin_id: DSD_ID, vfo: 1, config })],
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
        vfos={[vfo(1, { freqHz: 438_000_000 })]}
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
      status: parseStatus({
        status: {
          tool: READY,
          now: [{ ...CALL, src: 999, started_unix_ms: Date.now() }],
          last: null,
          calls: 2,
        },
      }),
      stale: false,
    };
    const store = await setup([
      ev(5, "dsd.call.start", { ...CALL, src: 999 }),
      ev(4, "dsd.call.end", { ...CALL, duration_ms: 8000, audio_ms: 7000, reason: "eos" }),
    ]);
    const dock = screen.getByTestId("digital-dock");
    expect(dock).toHaveAttribute("aria-label", "DMR decoder output");
    // Header + the one finished call; the start event is not a row of its own.
    const rows = within(dock).getAllByRole("row");
    expect(rows).toHaveLength(2);
    expect(rows[1]).toHaveTextContent("3101234 → TG 91");
    expect(rows[1]).toHaveTextContent("8.0 s");
    expect(within(dock).getByTestId("dsd-now-src")).toHaveTextContent("999");
    // The call on the air is also a badge in the header.
    expect(within(dock).getAllByText("999").length).toBeGreaterThanOrEqual(2);
    store.stop();
  });

  it("says that dsd-fme is needed, in the header and in the panel", async () => {
    polled = {
      status: parseStatus({ status: { tool: MISSING, now: [], last: null, calls: 0 } }),
      stale: false,
    };
    const store = await setup([], { protocol: "nxdn" });
    const dock = screen.getByTestId("digital-dock");
    expect(dock).toHaveAttribute("aria-label", "NXDN decoder output");
    expect(within(dock).getByText("needs dsd-fme")).toBeInTheDocument();
    expect(within(dock).getByTestId("dsd-needs-tool")).toHaveTextContent("Needs dsd-fme");
    expect(screen.getByTestId("dock-empty")).toHaveTextContent("Waiting for dsd-fme");
    store.stop();
  });
});

describe("useDsdStatus", () => {
  it("polls the status of the instance and goes stale when the poll fails", async () => {
    vi.useFakeTimers();
    const { useDsdStatus: real } =
      await vi.importActual<typeof import("./useDsdStatus")>("./useDsdStatus");
    const doc = status();
    const fetchStatus = vi
      .fn<(id: string, signal: AbortSignal) => Promise<DsdStatus>>()
      .mockResolvedValueOnce(doc)
      .mockRejectedValueOnce(new Error("down"));
    const { result } = renderHook(() => real("i-1", fetchStatus));
    expect(result.current.status).toEqual(EMPTY_STATUS);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(10);
    });
    expect(result.current.status.now[0]?.src).toBe(3101234);
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

function status(): DsdStatus {
  return parseStatus({ status: { tool: READY, now: [CALL], last: null, calls: 1 } });
}
