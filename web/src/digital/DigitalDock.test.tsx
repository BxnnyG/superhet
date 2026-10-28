import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { EventApi } from "../decoders/api";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { DecoderEventStore } from "../decoders/store";
import { PluginStore } from "../plugins/store";
import {
  APRS_ID,
  POCSAG_ID,
  RTL433_ID,
  aprs,
  fakePluginApi,
  instance,
  pocsag,
  rtl433,
} from "../plugins/testing";
import { vfo } from "../shell/fixtures";
import { toasts } from "../ui";
import { DOCK_POLL_MS, DigitalDock } from "./DigitalDock";

beforeEach(() => {
  localStorage.clear();
  toasts.clear();
});
afterEach(() => vi.useRealTimers());

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

function ev(id: number, kind: string, source: string, data: Record<string, unknown>, value = "x") {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "dev0",
    vfo: 1,
    freq_hz: 439_987_500,
    mode: "nfm",
    kind,
    source,
    value,
    meta: { instance: "i-1", data },
  });
  if (!e) throw new Error("fixture");
  return e;
}

const page = (id: number, text: string) =>
  ev(id, "pocsag.message", POCSAG_ID, {
    protocol: "pocsag",
    baud: 1200,
    address: 1234567,
    function: 3,
    kind: "alpha",
    message: text,
  });

interface Opts {
  events?: DecoderEvent[];
  instances?: Parameters<typeof fakePluginApi>[1];
  vfoOver?: Parameters<typeof vfo>[1];
  selected?: number | null;
}

async function setup(opts: Opts = {}) {
  const fake = fakePluginApi([pocsag, aprs, rtl433], opts.instances ?? [instance()]);
  const store = new PluginStore(fake.api);
  const eventApi = {
    list: vi.fn<EventApi["list"]>(() => Promise.resolve({ events: opts.events ?? [], next: null })),
    clear: vi.fn<EventApi["clear"]>(() => Promise.resolve(0)),
  } satisfies EventApi;
  const events = new DecoderEventStore({
    device: "dev0",
    api: eventApi,
    createSocket: () => ({ close() {} }) as unknown as WebSocket,
  });
  const vfos = [vfo(1, { freqHz: 439_987_500, ...opts.vfoOver })];
  const view = render(
    <DigitalDock
      plugins={store}
      device="dev0"
      vfos={vfos}
      selectedId={opts.selected === undefined ? 1 : opts.selected}
      createEvents={() => events}
    />,
  );
  store.start();
  await flush();
  return { ...fake, store, events, eventApi, view, vfos };
}

it("shows the decoded pages of this instance in a table, newest first", async () => {
  const other = ev(9, "pocsag.message", POCSAG_ID, { address: 7, message: "other" });
  other.meta.instance = "i-99";
  const { store } = await setup({ events: [page(3, "Fire alarm"), other, page(2, "Test page")] });
  const dock = screen.getByTestId("digital-dock");
  const rows = within(dock).getAllByRole("row");
  // Header + the two pages of this instance; the other instance's page is not shown.
  expect(rows).toHaveLength(3);
  expect(rows[1]).toHaveTextContent("RIC 1234567");
  expect(rows[1]).toHaveTextContent("Fire alarm");
  expect(rows[1]).toHaveTextContent("1200 baud");
  expect(rows[2]).toHaveTextContent("Test page");
  expect(within(dock).queryByText("other")).toBeNull();
  // Column heads of the mode.
  expect(within(dock).getByText("Address")).toBeInTheDocument();
  store.stop();
});

it("explains what to expect while nothing was decoded", async () => {
  const { store } = await setup({
    instances: [instance({ events_ok: 0, last_event_unix_ms: null })],
  });
  const empty = screen.getByTestId("dock-empty");
  expect(empty).toHaveTextContent("Listening");
  expect(empty).toHaveTextContent("No pages yet on 439.9875 MHz");
  expect(empty).toHaveTextContent("Pagers transmit only when somebody is called");
  // Samples arrive, so the panel does not blame the input.
  expect(screen.getByTestId("facts-input")).toHaveTextContent("441 kB");
  expect(within(screen.getByTestId("digital-dock")).queryByRole("alert")).toBeNull();
  store.stop();
});

it("says when no samples reach the decoder", async () => {
  const { store } = await setup({
    instances: [instance({ events_ok: 0, input_bytes: 0, input_level_db: null })],
  });
  const dock = screen.getByTestId("digital-dock");
  expect(within(dock).getByRole("alert")).toHaveTextContent("No samples have reached the decoder");
  expect(screen.getByTestId("facts-input")).toHaveTextContent("none yet");
  store.stop();
});

it("shows the failure of a crashed decoder with a restart button", async () => {
  const { api, store } = await setup({
    instances: [instance({ state: "error", last_error: "exited with exit status: 127" })],
  });
  const dock = screen.getByTestId("digital-dock");
  expect(within(dock).getByRole("alert")).toHaveTextContent("failed repeatedly");
  expect(within(dock).getByRole("alert")).toHaveTextContent("exit status: 127");
  fireEvent.click(within(dock).getByRole("button", { name: "Restart" }));
  await flush();
  expect(api.restart).toHaveBeenCalledWith("i-1");
  store.stop();
});

it("renders APRS and ISM output with their own columns", async () => {
  const aprsEvent = ev(1, "aprs.packet", APRS_ID, {
    source: "N0CALL-9",
    type: "position",
    path: ["WIDE1-1"],
    lat: 52.5,
    lon: 13.4,
    comment: "Hello",
  });
  const first = await setup({
    instances: [instance({ plugin_id: APRS_ID })],
    events: [aprsEvent],
    vfoOver: { freqHz: 144_800_000 },
  });
  let dock = screen.getByTestId("digital-dock");
  expect(within(dock).getByText("Station")).toBeInTheDocument();
  expect(dock).toHaveTextContent("N0CALL-9");
  expect(within(dock).getByRole("link", { name: "Open map" })).toBeInTheDocument();
  // APRS has no settings.
  expect(within(dock).queryByRole("button", { name: "Settings" })).toBeNull();
  first.store.stop();
  first.view.unmount();

  const ism = ev(2, "rtl433.reading", RTL433_ID, {
    model: "Nexus-TH",
    id: 165,
    channel: 2,
    temperature_C: 21.5,
    humidity: 48,
  });
  const second = await setup({
    instances: [instance({ plugin_id: RTL433_ID })],
    events: [ism],
    vfoOver: { freqHz: 433_920_000 },
  });
  dock = screen.getByTestId("digital-dock");
  expect(within(dock).getByText("Sensor")).toBeInTheDocument();
  expect(dock).toHaveTextContent("Nexus-TH #165");
  expect(dock).toHaveTextContent("21.5 °C");
  second.store.stop();
});

it("stops the decoder from the panel", async () => {
  const { api, store } = await setup();
  fireEvent.click(screen.getByRole("button", { name: "Stop POCSAG" }));
  await flush();
  expect(api.stop).toHaveBeenCalledWith("i-1");
  // The panel goes away with the instance.
  expect(screen.queryByTestId("digital-dock")).toBeNull();
  store.stop();
});

it("edits the settings inline and applies them without a restart of the instance", async () => {
  const { api, store } = await setup();
  const dock = screen.getByTestId("digital-dock");
  fireEvent.click(within(dock).getByRole("button", { name: "Settings" }));
  vi.useFakeTimers();
  fireEvent.change(within(dock).getByRole("combobox", { name: "Signal polarity" }), {
    target: { value: "inverted" },
  });
  await act(() => vi.advanceTimersByTimeAsync(700));
  vi.useRealTimers();
  expect(api.reconfigure).toHaveBeenCalledWith("i-1", { invert: true });
  expect(api.stop).not.toHaveBeenCalled();
  expect(api.start).not.toHaveBeenCalled();
  store.stop();
});

it("collapses to its header and remembers it", async () => {
  const { store, view } = await setup({ events: [page(1, "Visible")] });
  expect(screen.getByRole("table")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Collapse decoder output" }));
  expect(screen.queryByRole("table")).toBeNull();
  expect(screen.getByRole("button", { name: "Expand decoder output" })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
  view.unmount();
  store.stop();
});

it("renders nothing for an analog receiver or without a selection", async () => {
  const analog = await setup({ instances: [], selected: 1 });
  expect(screen.queryByTestId("digital-dock")).toBeNull();
  analog.store.stop();
  analog.view.unmount();
  const none = await setup({ selected: null });
  expect(screen.queryByTestId("digital-dock")).toBeNull();
  none.store.stop();
});

it("stops the decoder when the receiver is switched to an analog mode", async () => {
  const fake = fakePluginApi([pocsag], [instance()]);
  const store = new PluginStore(fake.api);
  const events = new DecoderEventStore({
    device: "dev0",
    api: {
      list: () => Promise.resolve({ events: [], next: null }),
      clear: () => Promise.resolve(0),
    },
    createSocket: () => ({ close() {} }) as unknown as WebSocket,
  });
  const props = { plugins: store, device: "dev0", selectedId: 1, createEvents: () => events };
  const { rerender } = render(<DigitalDock {...props} vfos={[vfo(1, { mode: "nfm" })]} />);
  store.start();
  await flush();
  expect(fake.api.stop).not.toHaveBeenCalled();
  // The user picks WFM on the mode bar.
  rerender(<DigitalDock {...props} vfos={[vfo(1, { mode: "wfm" })]} />);
  await flush();
  expect(fake.api.stop).toHaveBeenCalledWith("i-1");
  expect(toasts.getSnapshot().some((t) => t.title === "POCSAG stopped")).toBe(true);
  store.stop();
});

it("leaves a decoder alone that was started by hand on a non-NFM receiver", async () => {
  const fake = fakePluginApi([pocsag], [instance()]);
  const store = new PluginStore(fake.api);
  const events = new DecoderEventStore({
    device: "dev0",
    api: {
      list: () => Promise.resolve({ events: [], next: null }),
      clear: () => Promise.resolve(0),
    },
    createSocket: () => ({ close() {} }) as unknown as WebSocket,
  });
  render(
    <DigitalDock
      plugins={store}
      device="dev0"
      selectedId={1}
      createEvents={() => events}
      vfos={[vfo(1, { mode: "wfm" })]}
    />,
  );
  store.start();
  await flush();
  expect(fake.api.stop).not.toHaveBeenCalled();
  store.stop();
});

it("polls for fresh counters while it is on screen", async () => {
  vi.useFakeTimers();
  const { api, store } = await setupWithTimers();
  const calls = api.listInstances.mock.calls.length;
  await act(() => vi.advanceTimersByTimeAsync(DOCK_POLL_MS + 50));
  expect(api.listInstances.mock.calls.length).toBeGreaterThan(calls);
  store.stop();
});

async function setupWithTimers() {
  const fake = fakePluginApi([pocsag], [instance()]);
  const store = new PluginStore(fake.api);
  const events = new DecoderEventStore({
    device: "dev0",
    api: {
      list: () => Promise.resolve({ events: [], next: null }),
      clear: () => Promise.resolve(0),
    },
    createSocket: () => ({ close() {} }) as unknown as WebSocket,
  });
  render(
    <DigitalDock
      plugins={store}
      device="dev0"
      selectedId={1}
      createEvents={() => events}
      vfos={[vfo(1)]}
    />,
  );
  store.start();
  await act(() => vi.advanceTimersByTimeAsync(10));
  return { ...fake, store };
}
