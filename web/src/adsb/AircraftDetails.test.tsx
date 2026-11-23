// The aircraft panel: photo with credit, route with progress, charts, squawk meaning, "seen
// before", and every state of the online parts (off, loading, error, nothing found, paused).

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AuthContext, type AuthContextValue } from "../auth/context";
import { FULL_ACCESS } from "../auth/permissions";
import { AircraftDetails } from "./AircraftDetails";
import { InfoCache } from "./info";
import { parseStatus, tracked, type Tracked } from "./model";
import { SeriesStore } from "./series";
import {
  adsbStatusRaw,
  aircraftRaw,
  fakeAdsbApi,
  historyRaw,
  onlineFullRaw,
  onlineRaw,
} from "./testing";
import { SETTLE_MS } from "./useExtras";

const NOW = 1_790_000_000_000;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
  vi.setSystemTime(NOW);
});
afterEach(() => vi.useRealTimers());

function aircraft(over: Record<string, unknown> = {}): Tracked {
  const status = parseStatus(adsbStatusRaw({ aircraft: [aircraftRaw(over)] }));
  if (status === null) throw new Error("fixture");
  const [a] = tracked(status, NOW, NOW);
  if (a === undefined) throw new Error("fixture");
  return a;
}

const noInfo = () =>
  ({ aircraftInfo: () => null, operatorInfo: () => null }) as unknown as InfoCache;

const asAdmin = (admin: boolean, ui: ReactNode) => (
  <AuthContext.Provider
    value={
      { permissions: { ...FULL_ACCESS, isAdmin: admin, isOperator: admin } } as AuthContextValue
    }
  >
    {ui}
  </AuthContext.Provider>
);

function setup(opts: {
  admin?: boolean;
  online?: Record<string, unknown>;
  history?: Record<string, unknown> | null;
  a?: Tracked;
  series?: SeriesStore;
  now?: number;
}) {
  const { api, state } = fakeAdsbApi();
  if (opts.online !== undefined) state.online = opts.online;
  state.history = opts.history ?? null;
  const a = opts.a ?? aircraft();
  const series = opts.series ?? new SeriesStore();
  const ui = (
    <AircraftDetails
      a={a}
      units="metric"
      info={noInfo()}
      api={api}
      series={series}
      now={opts.now ?? NOW}
      follow={false}
      onFollow={() => undefined}
      onClose={() => undefined}
    />
  );
  const view = render(asAdmin(opts.admin ?? false, ui));
  return { api, state, view, a };
}

const settle = () => act(() => vi.advanceTimersByTimeAsync(SETTLE_MS + 10));

describe("online parts", () => {
  it("is quiet and local while online details are off, and lets an admin switch them on", async () => {
    const { api } = setup({ admin: true });
    await settle();
    const off = screen.getByTestId("adsb-online-off");
    expect(off).toHaveTextContent("Photo, route and full type name are off");
    // The local facts are all there.
    expect(screen.getByTestId("adsb-details")).toHaveTextContent("DLH123");
    expect(screen.queryByTestId("adsb-photo")).toBeNull();
    expect(screen.queryByTestId("adsb-route")).toBeNull();

    fireEvent.click(within(off).getByRole("button", { name: "Enable online details" }));
    const dialog = screen.getByRole("dialog");
    // What is sent and to whom is spelled out before anything is enabled.
    expect(dialog).toHaveTextContent("adsbdb.com");
    expect(dialog).toHaveTextContent("Planespotters.net");
    expect(dialog).toHaveTextContent("not the receiver position");
    expect(api.setOnlineSettings).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(api.setOnlineSettings).toHaveBeenCalledWith({
      enabled: true,
      adsbdb: true,
      planespotters: true,
    });
  });

  it("only points a non-admin to the admin console", async () => {
    setup({ admin: false });
    await settle();
    const off = screen.getByTestId("adsb-online-off");
    expect(off).toHaveTextContent("An admin can switch them on");
    expect(within(off).queryByRole("button")).toBeNull();
  });

  it("does not offer the switch when the server configuration locks it", async () => {
    setup({ admin: true, online: onlineRaw({ locked: true }) });
    await settle();
    const off = screen.getByTestId("adsb-online-off");
    expect(off).toHaveTextContent("keeps them off");
    expect(within(off).queryByRole("button")).toBeNull();
  });

  it("shows a loading state until the answer arrives", async () => {
    const { api } = setup({ online: onlineFullRaw() });
    // Before the settle delay nothing was asked yet.
    expect(screen.getByTestId("adsb-online-loading")).toBeInTheDocument();
    expect(api.online).not.toHaveBeenCalled();
    await settle();
    expect(api.online).toHaveBeenCalledWith("3c6444", "DLH123", expect.any(AbortSignal));
    expect(screen.queryByTestId("adsb-online-loading")).toBeNull();
  });

  it("shows photo with credit and link back, full type, owner and the route", async () => {
    setup({ online: onlineFullRaw() });
    await settle();
    const photo = screen.getByTestId("adsb-photo");
    const img = within(photo).getByRole("img");
    // The image URL is used unchanged, from the photo service's own host.
    expect(img).toHaveAttribute("src", "https://t.plnspttrs.net/05121/1053982_2810c547e6_280.jpg");
    expect(img.closest("a")).toHaveAttribute(
      "href",
      "https://www.planespotters.net/photo/1053982/d-aixp-lufthansa-airbus-a350-941",
    );
    expect(img.closest("a")?.getAttribute("rel") ?? "").not.toContain("nofollow");
    expect(photo).toHaveTextContent("Photo: Günther Feniuk");
    const identity = screen.getByTestId("adsb-identity");
    expect(identity).toHaveTextContent("Lufthansa");
    expect(identity).toHaveTextContent("Airbus A350 941 · D-AIXP");

    const route = screen.getByTestId("adsb-route");
    expect(route).toHaveTextContent("FRA");
    expect(route).toHaveTextContent("JFK");
    expect(route).toHaveTextContent("Frankfurt am Main Airport");
    expect(route).toHaveTextContent("New York, United States");
    // Marked as database data that may be wrong.
    expect(route).toHaveTextContent("It can be wrong");
    expect(screen.getByTestId("adsb-online-credit")).toHaveTextContent("adsbdb.com");
    expect(screen.getByTestId("adsb-online-credit")).toHaveTextContent("Planespotters.net");
  });

  it("measures progress along the great circle and warns when the position does not fit", async () => {
    // 51.1 N 7.0 E is near Frankfurt on the way to New York: early in the flight.
    setup({ online: onlineFullRaw(), a: aircraft({ lat: 51.1, lon: 7.0, speed_kt: 450 }) });
    await settle();
    const bar = screen.getByRole("progressbar", { name: "Progress along the route" });
    expect(Number(bar.getAttribute("aria-valuenow"))).toBeLessThan(10);
    expect(screen.getByTestId("adsb-route-progress")).toHaveTextContent("to go");
    expect(screen.getByTestId("adsb-route-progress")).toHaveTextContent("at this speed");
  });

  it("calls a route that does not fit the aircraft's position probably another flight", async () => {
    // Over Australia on a Frankfurt to New York route.
    setup({ online: onlineFullRaw(), a: aircraft({ lat: -25, lon: 135 }) });
    await settle();
    expect(screen.getByTestId("adsb-route-progress")).toHaveTextContent(
      "probably for another flight",
    );
  });

  it("shows no progress without a position but keeps the route", async () => {
    setup({
      online: onlineFullRaw(),
      a: aircraft({ lat: undefined, lon: undefined, seen_pos_s: undefined }),
    });
    await settle();
    expect(screen.getByTestId("adsb-route")).toBeInTheDocument();
    expect(screen.queryByTestId("adsb-route-progress")).toBeNull();
  });

  it("says so when there is no photo or no route", async () => {
    setup({
      online: onlineFullRaw({
        photo: null,
        route: null,
        sources: { adsbdb: "not_found", planespotters: "not_found" },
      }),
    });
    await settle();
    expect(screen.getByTestId("adsb-identity")).toHaveTextContent("No photo of this aircraft");
    expect(screen.getByTestId("adsb-identity")).toHaveTextContent("No route known for DLH123");
    expect(screen.queryByTestId("adsb-photo")).toBeNull();
  });

  it("reports a service that cannot be reached without losing the local facts", async () => {
    setup({
      online: onlineFullRaw({
        photo: null,
        route: null,
        aircraft: null,
        sources: { adsbdb: "unavailable", planespotters: "rate_limited" },
      }),
    });
    await settle();
    expect(screen.getByTestId("adsb-identity")).toHaveTextContent(
      "adsbdb.com could not be reached",
    );
    expect(screen.getByTestId("adsb-identity")).toHaveTextContent(
      "Planespotters.net is being asked too often",
    );
    expect(screen.getByTestId("adsb-details")).toHaveTextContent("Altitude (baro)");
  });

  it("shows an error with a retry when the request itself fails", async () => {
    const { api } = setup({ online: onlineFullRaw() });
    api.online.mockRejectedValueOnce(new Error("offline"));
    await settle();
    const err = screen.getByRole("alert");
    expect(err).toHaveTextContent("Online details could not be loaded");
    fireEvent.click(within(err).getByRole("button", { name: "Retry" }));
    await settle();
    expect(api.online).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId("adsb-photo")).toBeInTheDocument();
  });

  it("explains a photo the browser refused to load", async () => {
    setup({ online: onlineFullRaw() });
    await settle();
    fireEvent.error(within(screen.getByTestId("adsb-photo")).getByRole("img"));
    expect(screen.getByTestId("adsb-photo")).toHaveTextContent("reload the page once");
    // The credit stays.
    expect(screen.getByTestId("adsb-photo")).toHaveTextContent("Photo: Günther Feniuk");
  });
});

describe("local parts", () => {
  it("highlights an emergency squawk and says what it means", async () => {
    setup({ a: aircraft({ squawk: "7700" }) });
    await settle();
    const details = screen.getByTestId("adsb-details");
    expect(details).toHaveAttribute("data-emergency", "7700");
    expect(screen.getByTestId("adsb-squawk")).toHaveTextContent("Squawk 7700: General emergency");
    expect(screen.getByTestId("adsb-squawk").className).toContain("text-danger");
  });

  it("explains an ordinary squawk quietly and stays silent about unknown ones", async () => {
    const { view, a } = setup({ a: aircraft({ squawk: "7000" }) });
    await settle();
    expect(screen.getByTestId("adsb-squawk")).toHaveTextContent("VFR conspicuity");
    expect(screen.getByTestId("adsb-squawk").className).not.toContain("text-danger");
    view.unmount();
    setup({ a: { ...a, squawk: "4521" } });
    await settle();
    expect(screen.queryByTestId("adsb-squawk")).toBeNull();
  });

  it("draws the charts from the history of the last minutes and collects first", async () => {
    const series = new SeriesStore();
    const { view } = setup({ series });
    await settle();
    // One sample is not a line yet.
    expect(screen.getAllByText(/Collecting data/)).toHaveLength(3);
    view.unmount();

    for (let i = 0; i < 6; i++) {
      series.record(
        parseStatus(
          adsbStatusRaw({
            aircraft: [
              aircraftRaw({ alt_ft: 30000 + i * 500, speed_kt: 440 + i, vrate_fpm: 1000 }),
            ],
          }),
        )?.aircraft ?? [],
        NOW - (6 - i) * 10_000,
      );
    }
    setup({ series });
    await settle();
    const alt = screen.getByTestId("adsb-chart-altitude");
    expect(within(alt).getByRole("img").getAttribute("aria-label")).toContain("Altitude");
    expect(alt).not.toHaveTextContent("Collecting data");
    expect(screen.getByTestId("adsb-chart-ground-speed")).toBeInTheDocument();
    expect(screen.getByTestId("adsb-chart-vertical-rate")).toBeInTheDocument();
  });

  it("shows when the aircraft was seen before, from the local history", async () => {
    setup({ history: historyRaw() });
    await settle();
    const seen = screen.getByTestId("adsb-seen");
    expect(seen).toHaveTextContent("First heard");
    expect(seen).toHaveTextContent("14 history entries on 3 days");
    expect(seen).toHaveTextContent("Flown as DLH123 (9), DLH456 (5)");
  });

  it("says when the history does not know the aircraft", async () => {
    setup({ history: null });
    await settle();
    expect(screen.getByTestId("adsb-seen")).toHaveTextContent("Not in the decoder history yet");
  });

  it("works with local data only: no database, no online, no history", async () => {
    const { api } = setup({});
    await settle();
    expect(api.online).toHaveBeenCalled();
    const details = screen.getByTestId("adsb-details");
    expect(details).toHaveTextContent("Altitude (baro)");
    expect(details).toHaveTextContent("Squawk");
    expect(screen.getByTestId("adsb-seen")).toBeInTheDocument();
  });

  it("copies a summary that never contains the distance or bearing from the receiver", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const a = aircraft();
    expect(a.distKm).not.toBeNull();
    setup({ a, online: onlineFullRaw() });
    await settle();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy details" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(writeText).toHaveBeenCalledTimes(1);
    const text = (writeText.mock.calls[0] as unknown as [string])[0];
    expect(text).toContain("DLH123 (ICAO 3C6444)");
    expect(text).toContain("D-AIXP");
    expect(text).toContain("Route (database, may be wrong): FRA to JFK");
    expect(text).toContain("https://globe.adsbexchange.com/?icao=3c6444");
    expect(text).not.toMatch(/bearing|distance|receiver/i);
  });
});
