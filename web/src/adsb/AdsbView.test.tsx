import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fake, vfo } from "../shell/fixtures";
import type { Receiver } from "../shell/useReceiver";
import { AdsbView } from "./AdsbView";
import { AdsbStore } from "./store";
import { adsbReply, fakeAdsbApi } from "./testing";

// Leaflet needs a real layout; the map has its own test.
const mapProps = vi.fn();
vi.mock("./AdsbMap", () => ({
  AdsbMap: (props: { aircraft: { icao: string }[]; selected: string | null }) => {
    mapProps(props);
    return <div data-testid="map">{props.aircraft.map((a) => a.icao).join(",")}</div>;
  },
}));

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date", "setTimeout"] });
  vi.setSystemTime(1_790_000_000_000);
  mapProps.mockClear();
});
afterEach(() => vi.useRealTimers());

async function setup(running = true) {
  const { r } = fake([vfo(1)], 1);
  const api = fakeAdsbApi(adsbReply());
  const adsb = new AdsbStore(api.api);
  Object.assign(r, { adsb });
  adsb.start();
  if (running) adsb.follow("i-adsb");
  await act(() => vi.advanceTimersByTimeAsync(0));
  adsb.openView();
  return { r: r as Receiver, adsb };
}

/** Render and let the lazily loaded map resolve (timers are faked, so no findBy). */
async function show(r: Receiver) {
  const view = render(<AdsbView r={r} />);
  await act(() => vi.advanceTimersByTimeAsync(0));
  return view;
}

const rows = () =>
  screen
    .getAllByTestId(/^adsb-row-/)
    .map((tr) => tr.getAttribute("data-testid")?.replace("adsb-row-", ""));

describe("AdsbView", () => {
  it("lists the aircraft, newest first, and puts the ones with a position on the map", async () => {
    const { r } = await setup();
    await show(r);
    expect(screen.getByText("3 aircraft")).toBeInTheDocument();
    expect(screen.getByText("2 on map")).toBeInTheDocument();
    expect(screen.getByText("565 msg/s")).toBeInTheDocument();
    expect(rows()).toEqual(["3c6444", "406a3d", "4ca7b2"]);
    expect(screen.getByTestId("map")).toHaveTextContent("3c6444,406a3d");
    const dlh = screen.getByTestId("adsb-row-3c6444");
    expect(dlh).toHaveTextContent("DLH123");
    expect(dlh).toHaveTextContent("35,000 ft");
    expect(dlh).toHaveTextContent("451 kt");
    expect(screen.getByTestId("adsb-row-406a3d")).toHaveTextContent("ground");
    // No callsign: the address stands in, and the row says there is no position.
    expect(screen.getByTestId("adsb-row-4ca7b2")).toHaveTextContent(/no position/);
  });

  it("sorts by a column and flips the direction on a second click", async () => {
    const { r } = await setup();
    await show(r);
    fireEvent.click(screen.getByRole("button", { name: "Sort by Altitude" }));
    expect(rows()).toEqual(["3c6444", "406a3d", "4ca7b2"]);
    expect(screen.getByRole("columnheader", { name: /Altitude/ })).toHaveAttribute(
      "aria-sort",
      "descending",
    );
    fireEvent.click(screen.getByRole("button", { name: "Sort by Altitude" }));
    expect(rows()).toEqual(["406a3d", "3c6444", "4ca7b2"]);
    fireEvent.click(screen.getByRole("button", { name: "Sort by Callsign" }));
    expect(rows()).toEqual(["3c6444", "406a3d", "4ca7b2"]);
  });

  it("filters by text and by position", async () => {
    const { r } = await setup();
    await show(r);
    fireEvent.change(screen.getByRole("searchbox", { name: "Filter aircraft" }), {
      target: { value: "ezy" },
    });
    expect(rows()).toEqual(["406a3d"]);
    fireEvent.change(screen.getByRole("searchbox", { name: "Filter aircraft" }), {
      target: { value: "nothing like this" },
    });
    expect(screen.getByText("No aircraft match the filter.")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox", { name: "Filter aircraft" }), {
      target: { value: "" },
    });
    fireEvent.click(screen.getByRole("switch", { name: "Only with position" }));
    expect(rows()).toEqual(["3c6444", "406a3d"]);
  });

  it("selects an aircraft for the map and shows its details", async () => {
    const { r } = await setup();
    await show(r);
    fireEvent.click(screen.getByTestId("adsb-row-3c6444"));
    const details = screen.getByTestId("adsb-details");
    expect(details).toHaveTextContent("DLH123");
    expect(details).toHaveTextContent("-64 ft/min");
    expect(details).toHaveTextContent("1000");
    expect(mapProps).toHaveBeenLastCalledWith(expect.objectContaining({ selected: "3c6444" }));
    fireEvent.click(within(details).getByRole("button", { name: "Clear selection" }));
    expect(screen.queryByTestId("adsb-details")).toBeNull();
  });

  it("lets aircraft fade and leave the picture after 60 seconds", async () => {
    const { r } = await setup();
    await show(r);
    expect(screen.getByTestId("adsb-row-4ca7b2")).toHaveStyle({ opacity: "1" });
    // The status stays as it is (a stuck poll): the view ages the aircraft itself. Polling
    // continues, so stop it to see the ageing alone.
    r.adsb.stop();
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    const halfway = Number(screen.getByTestId("adsb-row-3c6444").style.opacity);
    expect(halfway).toBeLessThan(0.8);
    expect(halfway).toBeGreaterThan(0.6);
    await act(() => vi.advanceTimersByTimeAsync(28_000));
    // 3.1 s + 58 s: gone; the freshest ones (0.2 s, 0.5 s) are faint ghosts about to go.
    expect(screen.queryByTestId("adsb-row-4ca7b2")).toBeNull();
    expect(Number(screen.getByTestId("adsb-row-3c6444").style.opacity)).toBeLessThan(0.3);
    await act(() => vi.advanceTimersByTimeAsync(5_000));
    expect(screen.queryAllByTestId(/^adsb-row-/)).toHaveLength(0);
    expect(screen.getByText(/No aircraft heard yet/)).toBeInTheDocument();
  });

  it("closes with the button and with Escape", async () => {
    const { r, adsb } = await setup();
    await show(r);
    fireEvent.click(screen.getByRole("button", { name: "Close the ADS-B view" }));
    expect(adsb.getSnapshot().viewOpen).toBe(false);
    adsb.openView();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(adsb.getSnapshot().viewOpen).toBe(false);
  });

  it("says so when no instance runs", async () => {
    const { r } = await setup(false);
    await show(r);
    expect(screen.getByText("ADS-B is not running")).toBeInTheDocument();
    expect(screen.queryByTestId("map")).toBeNull();
  });
});
