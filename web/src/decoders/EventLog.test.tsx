import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { EventApi } from "./api";
import { EventLog } from "./EventLog";
import { parseEvent, type DecoderEvent } from "./events";
import { DecoderEventStore } from "./store";
import { toasts } from "../ui";

const ev = (id: number, over: Record<string, unknown> = {}): DecoderEvent => {
  const e = parseEvent({
    id,
    ts_ms: 1_700_000_000_000 + id * 1000,
    device: "rtl0",
    vfo: 2,
    freq_hz: 433_500_000,
    mode: "nfm",
    kind: "dtmf",
    value: `12${id}#`,
    meta: {},
    ...over,
  });
  if (!e) throw new Error("fixture");
  return e;
};

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
  const onTune = vi.fn();
  render(<EventLog store={store} device="rtl0" onTune={onTune} />);
  return { api, store, onTune };
}

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

beforeEach(() => toasts.clear());
afterEach(() => vi.useRealTimers());

it("lists the stored events newest first with time, frequency, kind and value", async () => {
  const { store } = setup([
    ev(2, { kind: "ctcss", value: "88.5 Hz", meta: { edge: "stop", duration_ms: 8000 } }),
    ev(1),
  ]);
  act(() => store.start());
  await flush();
  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(2);
  expect(rows[0]).toHaveTextContent("433.500");
  expect(rows[0]).toHaveTextContent("CTCSS");
  expect(rows[0]).toHaveTextContent("88.5 Hz");
  expect(rows[0]).toHaveTextContent("off after 8.0 s");
  expect(rows[1]).toHaveTextContent("DTMF");
  expect(rows[1]).toHaveTextContent("121#");
  expect(screen.getAllByTitle("VFO 2")).toHaveLength(2);
  store.stop();
});

it("shows an explicit empty state", async () => {
  const { store } = setup([]);
  act(() => store.start());
  await flush();
  expect(screen.getByText("Nothing heard yet")).toBeInTheDocument();
  store.stop();
});

it("clicking a row tunes to its frequency and mode", async () => {
  const { store, onTune } = setup([ev(1)]);
  act(() => store.start());
  await flush();
  fireEvent.click(screen.getByTitle("Tune to 433.500 MHz"));
  expect(onTune).toHaveBeenCalledWith({ freqHz: 433_500_000, mode: "nfm" });
  store.stop();
});

it("a kind chip filters on the server", async () => {
  const { api, store } = setup([ev(1)]);
  act(() => store.start());
  await flush();
  fireEvent.click(screen.getByRole("button", { name: "CTCSS/DCS" }));
  await flush();
  expect(api.list.mock.calls[1]?.[1]).toEqual({ kinds: ["ctcss", "dcs"], q: "" });
  expect(screen.getByRole("button", { name: "CTCSS/DCS" })).toHaveAttribute("aria-pressed", "true");
  store.stop();
});

it("searches after the typing pauses", async () => {
  vi.useFakeTimers();
  const { api, store } = setup([ev(1)]);
  act(() => store.start());
  await act(() => vi.advanceTimersByTimeAsync(0));
  fireEvent.change(screen.getByLabelText("Search decoder events"), { target: { value: "12" } });
  expect(api.list).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(300));
  expect(api.list).toHaveBeenCalledTimes(2);
  expect(api.list.mock.calls[1]?.[1]).toEqual({ kinds: [], q: "12" });
  store.stop();
});

it("the pause button toggles between Pause and Resume", async () => {
  const { store } = setup([ev(1)]);
  act(() => store.start());
  await flush();
  fireEvent.click(screen.getByRole("button", { name: /Pause/ }));
  expect(screen.getByRole("button", { name: /Resume/ })).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(screen.getByRole("button", { name: /Resume/ }));
  expect(screen.getByRole("button", { name: /Pause/ })).toBeInTheDocument();
  store.stop();
});

it("clearing asks first, then deletes the history", async () => {
  const { api, store } = setup([ev(1)]);
  act(() => store.start());
  await flush();
  fireEvent.click(screen.getByRole("button", { name: "More log actions" }));
  fireEvent.click(await screen.findByRole("menuitem", { name: "Clear history…" }));
  expect(api.clear).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Delete all" }));
  await flush();
  expect(api.clear).toHaveBeenCalledTimes(1);
  expect(screen.getByText("Nothing heard yet")).toBeInTheDocument();
  store.stop();
});

it("shows a retry when the history cannot be read", async () => {
  const api = {
    list: vi.fn<EventApi["list"]>(() => Promise.reject(new Error("HTTP 503"))),
    clear: vi.fn<EventApi["clear"]>(() => Promise.resolve(0)),
  };
  const store = new DecoderEventStore({
    device: "rtl0",
    api,
    createSocket: () => ({ close() {} }) as unknown as WebSocket,
  });
  render(<EventLog store={store} device="rtl0" onTune={vi.fn()} />);
  act(() => store.start());
  await flush();
  expect(screen.getByText(/The event history is unavailable: HTTP 503/)).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(api.list).toHaveBeenCalledTimes(2);
  store.stop();
});
