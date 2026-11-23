import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { RECEIVER_STORAGE_KEY } from "../adsb/receiver";
import { AuthContext, type AuthContextValue } from "../auth/context";
import { FULL_ACCESS } from "../auth/permissions";
import { parseManifest } from "../plugins/api";
import { fakePluginApi, ADSB_RAW } from "../plugins/testing";
import { PluginStore } from "../plugins/store";
import { fake, vfo } from "../shell/fixtures";
import type { Receiver } from "../shell/useReceiver";
import type { WeatherApi } from "./api";
import { WeatherError } from "./api";
import {
  parseCapture,
  parseConfig,
  parsePasses,
  WEATHER_PLUGIN_ID,
  type ElementsInfo,
} from "./model";
import { WeatherPanel } from "./WeatherPanel";

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
});

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

const WEATHER_RAW = {
  plugin: {
    ...ADSB_RAW.plugin,
    id: WEATHER_PLUGIN_ID,
    name: "Weather satellites (LRPT)",
    description: "Meteor-M LRPT",
  },
  runtime: {
    kind: "process",
    transport: "simple",
    command: ["superhet-plugin-adapter", "weather"],
  },
  input: { kind: "iq", sample_rate: 192000, format: "cs16", bandwidth_hz: 144000 },
  install: { acknowledge: { text: "Runs SatDump as a separate process." } },
};

const now = Date.now();
const PASS = (satellite: string, startMin: number, max: number) => ({
  satellite,
  norad_id: 59051,
  format: "LRPT",
  aos_ms: now + startMin * 60_000,
  tca_ms: now + (startMin + 5) * 60_000,
  los_ms: now + (startMin + 10) * 60_000,
  max_elevation_deg: max,
  aos_azimuth_deg: 10,
  tca_azimuth_deg: 90,
  los_azimuth_deg: 200,
  in_progress: false,
  downlinks: [
    { hz: 137_900_000, label: "LRPT 137.900 MHz" },
    { hz: 137_100_000, label: "LRPT 137.100 MHz" },
  ],
  elements_age_h: 4,
});

const ELEMENTS = {
  enabled: false,
  source: null,
  fetched_ms: null,
  satellites: [],
  download: {
    name: "CelesTrak",
    url: "https://celestrak.org/x",
    info_url: "https://celestrak.org/",
  },
};

const CONFIG = {
  satellites: [
    {
      norad_id: 59051,
      name: "METEOR-M2 4",
      format: "LRPT",
      status: "operational",
      note: "",
      downlinks: [],
    },
    {
      norad_id: 57166,
      name: "METEOR-M2 3",
      format: "LRPT",
      status: "operational",
      note: "",
      downlinks: [],
    },
    {
      norad_id: 33591,
      name: "NOAA 19",
      format: "APT",
      status: "decommissioned",
      note: "Decommissioned by NOAA on 2025-08-13 (battery failure); transmitters shut down.",
      downlinks: [],
    },
  ],
  checked: "2026-11-23",
  elements: ELEMENTS,
  store: { available: true, captures: 1, used_bytes: 4096, max_bytes: 0 },
};

const CAPTURE = {
  id: "1700000100-0001",
  satellite: "METEOR-M2 4",
  pipeline: "lrpt",
  started_ms: 1_700_000_100_000,
  duration_s: 600,
  frequency_hz: 137_900_000,
  max_elevation_deg: 56.4,
  peak_db: 7.5,
  keep: false,
  size_bytes: 2048,
  images: [
    { name: "rgb.png", label: "rgb 221", width: 100, height: 50, size: 10, thumb: "thumb-rgb.png" },
    { name: "ch1.png", label: "ch1", width: 100, height: 50, size: 10, thumb: null },
  ],
};

function fakeApi(over: Partial<WeatherApi> = {}) {
  const passes = vi.fn<WeatherApi["passes"]>(() =>
    Promise.resolve(
      parsePasses({
        position_from: "request",
        passes: [PASS("METEOR-M2 4", 25, 56.4), PASS("METEOR-M2 3", 140, 12)],
        elements: { source: "download", fetched_ms: 1 },
      }),
    ),
  );
  const api: WeatherApi = {
    config: vi.fn(() => Promise.resolve(parseConfig(CONFIG))),
    passes,
    captures: vi.fn(() =>
      Promise.resolve({
        captures: [parseCapture(CAPTURE)].filter((c) => c !== null),
        nextBeforeMs: null,
      }),
    ),
    keep: vi.fn(() =>
      Promise.resolve(parseCapture({ ...CAPTURE, keep: true }) ?? Promise.reject()),
    ),
    remove: vi.fn(() => Promise.resolve()),
    setDownload: vi.fn(() =>
      Promise.resolve({ ...parseConfig({ elements: { ...ELEMENTS, enabled: true } }).elements }),
    ),
    refresh: vi.fn(() => Promise.resolve({} as ElementsInfo)),
    paste: vi.fn(() => Promise.resolve({} as ElementsInfo)),
    ...over,
  };
  return { api, passes };
}

const asRole = (ui: ReactNode, perms: Partial<typeof FULL_ACCESS>) => (
  <AuthContext.Provider value={{ permissions: { ...FULL_ACCESS, ...perms } } as AuthContextValue}>
    {ui}
  </AuthContext.Provider>
);

async function setup(opts: { installed?: boolean } = {}) {
  const { r } = fake([vfo(1)], 1);
  const plugins = fakePluginApi(opts.installed === false ? [] : [parseManifest(WEATHER_RAW)], []);
  const store = new PluginStore(plugins.api);
  store.start();
  Object.assign(r, { plugins: store, tuneTo: vi.fn() });
  await flush();
  return { r: r as Receiver, plugins };
}

describe("WeatherPanel", () => {
  it("says which satellites can be received and which were switched off", async () => {
    const { r } = await setup();
    const { api } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, {}));
    await flush();
    expect(screen.getByText(/Receivable now:/).parentElement).toHaveTextContent(
      "METEOR-M2 4, METEOR-M2 3",
    );
    expect(screen.getByText(/Not receivable: NOAA 19/)).toBeInTheDocument();
    expect(screen.getByText(/Decommissioned by NOAA on 2025-08-13/)).toBeInTheDocument();
    expect(screen.getByText(/Status checked 2026-11-23/)).toBeInTheDocument();
  });

  it("lists the passes with countdown, elevation and frequency", async () => {
    const { r } = await setup();
    const { api, passes } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, {}));
    await flush();
    const rows = screen.getAllByTestId("pass-row");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]!).getByText("METEOR-M2 4")).toBeInTheDocument();
    expect(within(rows[0]!).getByText(/in 25 min/)).toBeInTheDocument();
    expect(within(rows[0]!).getByText("56° max")).toBeInTheDocument();
    expect(within(rows[0]!).getByText(/137\.9\d* MHz LRPT/)).toBeInTheDocument();
    expect(passes).toHaveBeenCalledWith(expect.objectContaining({ dev: "dev0", hours: 24 }));
    // No position typed: the server's (ADS-B) position is used, none is sent.
    expect(passes.mock.calls[0]?.[0]).not.toHaveProperty("lat");
  });

  it("sends the typed position only after Use and remembers it with ADS-B's", async () => {
    const { r } = await setup();
    const { api, passes } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, {}));
    await flush();
    fireEvent.change(screen.getByLabelText("Latitude"), { target: { value: "51.0" } });
    fireEvent.change(screen.getByLabelText("Longitude"), { target: { value: "7,5" } });
    expect(passes).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Use" }));
    await flush();
    expect(passes).toHaveBeenCalledTimes(2);
    expect(passes.mock.calls[1]?.[0]).toMatchObject({ lat: 51, lon: 7.5 });
    expect(JSON.parse(window.localStorage.getItem(RECEIVER_STORAGE_KEY) ?? "{}")).toEqual({
      lat: "51.0",
      lon: "7,5",
    });
  });

  it("explains a missing position or orbit data instead of failing", async () => {
    const { r } = await setup();
    const { api } = fakeApi({
      passes: vi.fn(() =>
        Promise.reject(
          new WeatherError(
            "There are no orbit data yet. An admin can allow the download below or paste the data.",
            409,
            "no_elements",
          ),
        ),
      ),
    });
    render(asRole(<WeatherPanel r={r} api={api} />, {}));
    await flush();
    expect(screen.getByText(/There are no orbit data yet/)).toBeInTheDocument();
    expect(screen.queryAllByTestId("pass-row")).toHaveLength(0);
  });

  it("tunes the receiver to the satellite and starts the decoder (after the notice)", async () => {
    const { r, plugins } = await setup();
    const { api } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, {}));
    await flush();
    fireEvent.click(screen.getAllByRole("button", { name: /Tune to 137\.9/ })[0]!);
    await flush();
    // The notice comes first.
    expect(screen.getByText("Runs SatDump as a separate process.")).toBeInTheDocument();
    expect(plugins.api.start).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await flush();
    expect(plugins.api.start).toHaveBeenCalledWith(
      expect.objectContaining({
        pluginId: WEATHER_PLUGIN_ID,
        device: "dev0",
        vfo: 1,
        freqHz: 137_900_000,
      }),
    );
    // The VFO got the wide preset on the satellite's frequency.
    expect(r.vfoCtl.update).toHaveBeenCalledWith(
      1,
      expect.objectContaining({ freqHz: 137_900_000, bandwidthHz: 144_000, mode: "nfm" }),
    );
  });

  it("offers no Tune to listeners and says when the plugin is missing", async () => {
    const { r } = await setup({ installed: false });
    const { api } = fakeApi();
    const { unmount } = render(asRole(<WeatherPanel r={r} api={api} />, {}));
    await flush();
    expect(screen.getAllByText(/weather decoder plugin is not installed/)).toHaveLength(2);
    expect(screen.queryByRole("button", { name: /Tune to/ })).toBeNull();
    unmount();
    const ok = await setup();
    render(asRole(<WeatherPanel r={ok.r} api={api} />, { canUsePlugins: false }));
    await flush();
    expect(screen.queryByRole("button", { name: /Tune to/ })).toBeNull();
    expect(screen.getAllByTestId("pass-row")).toHaveLength(2);
  });

  it("shows the gallery, opens an image and keeps or deletes it (operators)", async () => {
    const { r } = await setup();
    const { api } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, { isOperator: true }));
    await flush();
    const thumb = screen.getByAltText(/METEOR-M2 4/);
    expect(thumb).toHaveAttribute(
      "src",
      "/api/v1/weather/captures/1700000100-0001/images/rgb.png?thumb=true",
    );
    expect(within(screen.getByTestId("capture")).getByText(/56° max/)).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("capture").querySelector("button")!);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByAltText("rgb 221")).toHaveAttribute(
      "src",
      "/api/v1/weather/captures/1700000100-0001/images/rgb.png",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: "ch1" }));
    expect(within(dialog).getByAltText("ch1")).toBeInTheDocument();
    expect(within(dialog).getByRole("link", { name: /Download PNG/ })).toHaveAttribute(
      "href",
      "/api/v1/weather/captures/1700000100-0001/images/ch1.png?download=true",
    );
    fireEvent.click(within(dialog).getByRole("button", { name: /Keep/ }));
    await flush();
    expect(api.keep).toHaveBeenCalledWith("1700000100-0001", true);
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await flush();
    expect(api.remove).toHaveBeenCalledWith("1700000100-0001");
  });

  it("hides keep and delete from listeners and the orbit controls from non-admins", async () => {
    const { r } = await setup();
    const { api } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, { isOperator: false, isAdmin: false }));
    await flush();
    fireEvent.click(screen.getByTestId("capture").querySelector("button")!);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).queryByRole("button", { name: /Keep/ })).toBeNull();
    expect(within(dialog).queryByRole("button", { name: "Delete" })).toBeNull();
    expect(screen.queryByText("Orbit data")).toBeNull();
  });

  it("lets an admin switch the download on, with the terms in plain words", async () => {
    const { r } = await setup();
    const { api } = fakeApi();
    render(asRole(<WeatherPanel r={r} api={api} />, { isAdmin: true, isOperator: true }));
    await flush();
    expect(screen.getByText(/at most every 2 hours/)).toBeInTheDocument();
    expect(screen.getByText(/without your position or any account data/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("switch", { name: /Download from CelesTrak automatically/ }));
    await flush();
    expect(api.setDownload).toHaveBeenCalledWith(true);
    fireEvent.change(screen.getByLabelText(/Paste TLE or OMM/), { target: { value: "1 2\n3 4" } });
    fireEvent.click(screen.getByRole("button", { name: "Use pasted data" }));
    await flush();
    expect(api.paste).toHaveBeenCalledWith("1 2\n3 4");
  });
});
