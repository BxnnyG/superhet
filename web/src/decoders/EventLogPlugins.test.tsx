import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { toasts } from "../ui";
import type { EventApi } from "./api";
import { EventLog } from "./EventLog";
import { parseEvent, type DecoderEvent } from "./events";
import { DecoderEventStore } from "./store";

const POCSAG = "org.superhet.decoder.pocsag";

const ev = (id: number, over: Record<string, unknown> = {}): DecoderEvent => {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "rtl0",
    vfo: 2,
    freq_hz: 433_500_000,
    mode: "nfm",
    kind: "dtmf",
    source: "core",
    value: `12${id}#`,
    meta: {},
    ...over,
  });
  if (!e) throw new Error("fixture");
  return e;
};

const pocsag = (id: number) =>
  ev(id, {
    kind: "pocsag.message",
    source: POCSAG,
    vfo: null,
    value: "1234567: Fire alarm",
    meta: {
      instance: "i-1",
      data: {
        protocol: "pocsag",
        baud: 1200,
        address: 1234567,
        function: 3,
        kind: "alpha",
        message: "Fire alarm",
        summary: "1234567: Fire alarm",
      },
    },
  });

function setup(events: DecoderEvent[]) {
  const api = {
    list: vi.fn<EventApi["list"]>(() => Promise.resolve({ events, next: null })),
    clear: vi.fn<EventApi["clear"]>(() => Promise.resolve(events.length)),
  } satisfies EventApi;
  const store = new DecoderEventStore({
    device: "rtl0",
    api,
    createSocket: () => ({ close() {} }) as unknown as WebSocket,
  });
  render(<EventLog store={store} device="rtl0" onTune={vi.fn()} />);
  return { api, store };
}

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

beforeEach(() => toasts.clear());
afterEach(() => vi.useRealTimers());

it("renders plugin events as two-line rows and keeps a null VFO", async () => {
  const { store } = setup([pocsag(2), ev(1)]);
  act(() => store.start());
  await flush();
  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(2);
  expect(rows[0]).toHaveTextContent("POCSAG");
  expect(rows[0]).toHaveTextContent("RIC 1234567");
  expect(rows[0]).toHaveTextContent("Fire alarm");
  expect(rows[0]).toHaveTextContent("1200 baud");
  // Plugin rows are taller than the compact rows of the built-in detectors.
  expect(rows[0]?.style.height).toBe("52px");
  expect(rows[1]?.style.height).toBe("32px");
  expect(rows[0]?.querySelector('[title^="VFO"]')).toBeNull();
  store.stop();
});

it("opens the details of a plugin event with its fields and raw data", async () => {
  const { store } = setup([pocsag(2)]);
  act(() => store.start());
  await flush();
  fireEvent.click(screen.getByRole("button", { name: "Event details" }));
  const dialog = screen.getByRole("dialog");
  expect(dialog).toHaveTextContent("Address (RIC)");
  expect(dialog).toHaveTextContent("1234567");
  expect(dialog).toHaveTextContent("alphanumeric");
  expect(dialog).toHaveTextContent("Raw data");
  fireEvent.click(screen.getAllByRole("button", { name: "Close" })[0] as HTMLElement);
  expect(screen.queryByRole("dialog")).toBeNull();
  store.stop();
});

it("filters by source on the server and offers the sources seen", async () => {
  const { api, store } = setup([pocsag(2), ev(1)]);
  act(() => store.start());
  await flush();
  const select = screen.getByRole("combobox", { name: "Source" });
  expect(select).toHaveTextContent("Built-in detectors");
  fireEvent.change(select, { target: { value: POCSAG } });
  await flush();
  expect(api.list.mock.calls.at(-1)?.[1]).toEqual({ kinds: [], source: POCSAG, q: "" });
  // The plugin's event types become the chips.
  expect(screen.getByRole("button", { name: "pocsag.message" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "DTMF" })).toBeNull();
  store.stop();
});

it("shows an unknown plugin event kind generically", async () => {
  const { store } = setup([
    ev(1, {
      kind: "x.thing",
      source: "org.example.x",
      vfo: null,
      value: "something happened",
      meta: { data: { level: 3, ok: true } },
    }),
  ]);
  act(() => store.start());
  await flush();
  const row = screen.getByRole("listitem");
  expect(row).toHaveTextContent("x.thing");
  expect(row).toHaveTextContent("something happened");
  store.stop();
});
