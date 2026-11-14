// The aircraft database and range statistics in the ADS-B overlay (docs/plugins-adsb.md).

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthContext, type AuthContextValue } from "../auth/context";
import { FULL_ACCESS } from "../auth/permissions";
import { fake, vfo } from "../shell/fixtures";
import type { Receiver } from "../shell/useReceiver";
import { AdsbView } from "./AdsbView";
import { parseDatabaseStatus } from "./info";
import { AdsbStore } from "./store";
import { adsbReply, databaseStatusRaw, fakeAdsbApi } from "./testing";

const mapProps = vi.fn();
vi.mock("./AdsbMap", () => ({
  AdsbMap: (props: {
    aircraft: { icao: string }[];
    symbols?: ReadonlyMap<string, string>;
    coverage?: unknown[] | null;
  }) => {
    mapProps(props);
    return <div data-testid="map" />;
  },
}));

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "Date", "setTimeout"] });
  vi.setSystemTime(1_790_000_000_000);
  mapProps.mockClear();
});
afterEach(() => vi.useRealTimers());

const asListener = (ui: ReactNode) => (
  <AuthContext.Provider
    value={
      {
        permissions: { ...FULL_ACCESS, isOperator: false, canUsePlugins: false, canTune: false },
      } as AuthContextValue
    }
  >
    {ui}
  </AuthContext.Provider>
);

async function show(wrap: (ui: ReactNode) => ReactNode = (ui) => ui) {
  const { r } = fake([vfo(1)], 1);
  const fakeApi = fakeAdsbApi(adsbReply());
  const adsb = new AdsbStore(fakeApi.api);
  Object.assign(r, { adsb });
  adsb.start();
  adsb.follow("i-adsb");
  await act(() => vi.advanceTimersByTimeAsync(0));
  adsb.openView();
  render(wrap(<AdsbView r={r as Receiver} />));
  // Lazy map, debounced lookup, then the answer.
  await act(() => vi.advanceTimersByTimeAsync(500));
  return { adsb, api: fakeApi.api, state: fakeApi.state };
}

describe("aircraft database in the ADS-B view", () => {
  it("asks the local lookup once for the aircraft and the airlines in view", async () => {
    const { api } = await show();
    const calls = api.lookup.mock.calls as unknown as [string[], string[]][];
    expect(calls).toHaveLength(1);
    expect(calls[0]?.[0]).toEqual(["3c6444", "406a3d", "4ca7b2"]);
    expect(calls[0]?.[1]).toEqual(["DLH", "EZY"]);
  });

  it("shows registration, type, airline and the flag in the table", async () => {
    await show();
    const row = screen.getByTestId("adsb-row-3c6444");
    expect(row).toHaveTextContent("D-AIXA");
    expect(row).toHaveTextContent("A359");
    expect(row).toHaveTextContent("Lufthansa");
    expect(within(row).getByTestId("adsb-flag-3c6444")).toHaveAttribute("title", "Germany");
    // Not in the database: callsign and address only, still a flag from the address block.
    const other = screen.getByTestId("adsb-row-4ca7b2");
    expect(other).not.toHaveTextContent("A359");
    expect(within(other).getByTestId("adsb-flag-4ca7b2")).toHaveAttribute("title", "Ireland");
  });

  it("picks the map symbol from the type class and the emitter category", async () => {
    await show();
    const props = mapProps.mock.calls.at(-1)?.[0] as { symbols: Map<string, string> };
    // A350 (L2J, heavy wake) and A320 (L2J, medium) from the database; no data and no category.
    expect(props.symbols.get("3c6444")).toBe("wide");
    expect(props.symbols.get("406a3d")).toBe("narrow");
    expect(props.symbols.get("4ca7b2")).toBe("unknown");
  });

  it("shows the details with a photo link out and the licence note", async () => {
    await show();
    fireEvent.click(screen.getByTestId("adsb-row-3c6444"));
    const details = screen.getByTestId("adsb-details");
    expect(details).toHaveTextContent("D-AIXA");
    expect(details).toHaveTextContent("AIRBUS A-350-900");
    expect(details).toHaveTextContent("Lufthansa");
    expect(details).toHaveTextContent("Germany");
    expect(details).toHaveTextContent("Wide-body airliner");
    const spotters = within(details).getByRole("link", { name: "Planespotters" });
    expect(spotters).toHaveAttribute("href", "https://www.planespotters.net/hex/3C6444");
    expect(spotters).toHaveAttribute("target", "_blank");
    expect(spotters).toHaveAttribute("rel", expect.stringContaining("noreferrer"));
    expect(within(details).getByRole("link", { name: "JetPhotos" })).toHaveAttribute(
      "href",
      "https://www.jetphotos.com/registration/D-AIXA",
    );
    const note = screen.getByTestId("adsb-db-note");
    expect(note).toHaveTextContent("Mictronics aircraft database");
    expect(within(note).getByRole("link", { name: "ODC-By 1.0" })).toHaveAttribute(
      "href",
      "https://opendatacommons.org/licenses/by/1-0/",
    );
  });

  it("explains the empty state when no database is imported", async () => {
    const { r } = fake([vfo(1)], 1);
    const fakeApi = fakeAdsbApi(adsbReply());
    const empty = parseDatabaseStatus(databaseStatusRaw({ database: null }));
    if (empty === null) throw new Error("fixture");
    fakeApi.state.database = empty;
    const adsb = new AdsbStore(fakeApi.api);
    Object.assign(r, { adsb });
    adsb.start();
    adsb.follow("i-adsb");
    await act(() => vi.advanceTimersByTimeAsync(0));
    render(<AdsbView r={r as Receiver} />);
    await act(() => vi.advanceTimersByTimeAsync(500));
    expect(screen.getByTestId("adsb-db-note")).toHaveTextContent("No aircraft database imported");
  });
});

describe("range statistics in the ADS-B view", () => {
  it("shows the polar plot, the farthest reception and the history", async () => {
    const { api } = await show();
    fireEvent.click(screen.getByRole("radio", { name: "Range" }));
    expect(api.range).toHaveBeenCalledWith("dev0", 24, expect.anything());
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByTestId("adsb-polar")).toBeInTheDocument();
    expect(screen.getByTestId("adsb-range-best")).toHaveTextContent("210 km towards E");
    expect(screen.getByTestId("adsb-range-best")).toHaveTextContent("9,100 to 12,200 m");
    expect(screen.getByRole("img", { name: /Messages per second/ })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /Aircraft from/ })).toBeInTheDocument();
    expect(screen.getByRole("list", { name: /per altitude band/ })).toHaveTextContent("95.3 km");
  });

  it("draws the coverage outline on the map when asked", async () => {
    await show();
    fireEvent.click(screen.getByRole("radio", { name: "Range" }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect((mapProps.mock.calls.at(-1)?.[0] as { coverage: unknown }).coverage).toBeNull();
    fireEvent.click(screen.getByRole("switch", { name: "Coverage outline on the map" }));
    const coverage = (mapProps.mock.calls.at(-1)?.[0] as { coverage: unknown[] }).coverage;
    expect(coverage).toHaveLength(72);
  });

  it("resets after a confirmation (operators)", async () => {
    const { api } = await show();
    fireEvent.click(screen.getByRole("radio", { name: "Range" }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(api.resetRange).not.toHaveBeenCalled();
    const dialog = screen.getByRole("dialog", { name: "Reset range statistics?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(api.resetRange).toHaveBeenCalledWith("dev0");
    // The statistics are loaded again.
    expect(api.range.mock.calls.length).toBeGreaterThan(1);
  });

  it("offers no reset to listeners", async () => {
    await show(asListener);
    fireEvent.click(screen.getByRole("radio", { name: "Range" }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByTestId("adsb-polar")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
  });

  it("says what is missing without a receiver position or data", async () => {
    const { api } = await show();
    api.range.mockImplementation(() =>
      Promise.resolve({
        device: "dev0",
        receiver: null,
        sinceMs: null,
        cells: [],
        bucketS: 60,
        series: [],
      }),
    );
    fireEvent.click(screen.getByRole("radio", { name: "Range" }));
    await act(() => vi.advanceTimersByTimeAsync(0));
    // A different history length loads again, now with the empty answer.
    fireEvent.change(screen.getByLabelText("History length"), { target: { value: "6" } });
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(screen.getByText(/Range needs the receiver position/)).toBeInTheDocument();
    expect(screen.getByText(/No positions recorded yet/)).toBeInTheDocument();
  });
});
