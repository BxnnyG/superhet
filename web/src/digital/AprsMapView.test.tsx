import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { parseEvent, type DecoderEvent } from "../decoders/events";
import { AprsMapView } from "./AprsMapView";

// Leaflet needs a real layout; the map has its own test. The tiles never leave the test: the
// map config falls back to its default without a server and the (mocked) map loads nothing.
const mapProps = vi.fn();
vi.mock("./AprsMap", () => ({
  AprsMap: (props: { stations: { id: string }[]; selected: string | null }) => {
    mapProps(props);
    return <div data-testid="map">{props.stations.map((s) => s.id).join(",")}</div>;
  },
}));

const NOW = 1_790_000_000_000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date"] });
  vi.setSystemTime(NOW);
  mapProps.mockClear();
});
afterEach(() => vi.useRealTimers());

let nextId = 1;
const packet = (agoMs: number, data: Record<string, unknown>): DecoderEvent => {
  const e = parseEvent({
    id: nextId++,
    ts_ms: NOW - agoMs,
    device: "dev0",
    vfo: 1,
    freq_hz: 144_800_000,
    mode: null,
    kind: "aprs.packet",
    source: "org.superhet.decoder.aprs",
    value: "",
    meta: { instance: "i-1", data: { dest: "APRS", path: ["WIDE1-1"], info: "", ...data } },
  });
  if (!e) throw new Error("fixture");
  return e;
};

const flush = () => act(() => vi.advanceTimersByTimeAsync(0));

it("lists the stations that sent a position, newest first, and draws them on the map", async () => {
  render(
    <AprsMapView
      events={[
        packet(120_000, { source: "OLD-1", lat: 52.5, lon: 13.4, symbol: "/>" }),
        packet(5_000, { source: "NEW-2", lat: 52.6, lon: 13.5, symbol: "/-" }),
        packet(1_000, { source: "MSG-3", type: "message", message: "hi" }),
      ]}
    />,
  );
  await flush();
  const list = within(screen.getByRole("region", { name: "APRS stations" }));
  expect(list.getAllByRole("button").map((b) => b.textContent)).toEqual([
    "NEW-2just now",
    "OLD-12 min ago",
  ]);
  expect(screen.getByTestId("map")).toHaveTextContent("NEW-2,OLD-1");
});

it("shows the details of a station when it is picked and hides them again", async () => {
  render(
    <AprsMapView
      events={[
        packet(60_000, { source: "N0CALL-9", lat: 52.5, lon: 13.4, comment: "On the road" }),
        packet(10_000, {
          source: "N0CALL-9",
          lat: 52.51,
          lon: 13.41,
          symbol: "/>",
          speed_kn: 35,
          course: 90,
          comment: "Still on the road",
        }),
      ]}
    />,
  );
  await flush();
  expect(screen.queryByTestId("aprs-details")).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: /N0CALL-9/ }));
  const details = screen.getByTestId("aprs-details");
  expect(details).toHaveTextContent("2 packets");
  expect(details).toHaveTextContent("52.51000, 13.41000");
  expect(details).toHaveTextContent("35 kn");
  expect(details).toHaveTextContent("Still on the road");
  expect(details).toHaveTextContent("WIDE1-1");
  expect(within(details).getByRole("link", { name: "Open map" })).toHaveAttribute(
    "rel",
    "noopener noreferrer",
  );
  expect(mapProps).toHaveBeenLastCalledWith(expect.objectContaining({ selected: "N0CALL-9" }));
  fireEvent.click(within(details).getByRole("button", { name: "Close details" }));
  expect(screen.queryByTestId("aprs-details")).toBeNull();
});

it("says so while no station has a position", async () => {
  render(<AprsMapView events={[packet(1000, { source: "A1", type: "status" })]} />);
  await flush();
  expect(screen.getByText("No station with a position heard yet.")).toBeInTheDocument();
});
