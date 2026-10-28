import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AuthContext, type AuthContextValue } from "../auth/context";
import { FULL_ACCESS } from "../auth/permissions";
import { ADSB_RAW, fakePluginApi, instance } from "../plugins/testing";
import { parseManifest, type PluginInstance } from "../plugins/api";
import { PluginStore } from "../plugins/store";
import { fake, vfo } from "../shell/fixtures";
import type { Receiver } from "../shell/useReceiver";
import { AdsbPanel } from "./AdsbPanel";
import { ADSB_PLUGIN_ID } from "./model";
import { RECEIVER_STORAGE_KEY } from "./receiver";
import { AdsbStore } from "./store";
import { adsbReply, adsbStatusRaw, fakeAdsbApi } from "./testing";

beforeEach(() => {
  vi.clearAllMocks();
  window.localStorage.clear();
});

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

const ours = parseManifest({ ...ADSB_RAW, plugin: { ...ADSB_RAW.plugin, id: ADSB_PLUGIN_ID } });

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

async function setup(
  opts: {
    running?: boolean;
    installed?: boolean;
    status?: Record<string, unknown>;
    instances?: PluginInstance[];
  } = {},
) {
  const { r } = fake([vfo(1)], 1);
  const api = fakeAdsbApi(adsbReply(opts.status ? { status: adsbStatusRaw(opts.status) } : {}));
  const adsb = new AdsbStore(api.api);
  const pluginStore = new PluginStore(
    fakePluginApi(opts.installed === false ? [] : [ours], opts.instances ?? []).api,
  );
  Object.assign(r, { adsb, plugins: pluginStore });
  adsb.start();
  pluginStore.start();
  if (opts.running !== false) adsb.follow("i-adsb");
  await flush();
  return { r: r as Receiver, adsb, pluginStore };
}

describe("AdsbPanel", () => {
  it("shows the headline numbers and opens the map", async () => {
    const { r, adsb } = await setup();
    render(<AdsbPanel r={r} />);
    const summary = screen.getByTestId("adsb-summary");
    expect(within(summary).getByText("Aircraft").nextSibling).toHaveTextContent("3");
    expect(within(summary).getByText("With position").nextSibling).toHaveTextContent("2");
    expect(within(summary).getByText("Messages/s").nextSibling).toHaveTextContent("565");
    fireEvent.click(screen.getByRole("button", { name: "Open the aircraft map and table" }));
    expect(adsb.getSnapshot().viewOpen).toBe(true);
  });

  it("waits for the decoder while it starts and says when nothing was heard", async () => {
    const { r } = await setup({ status: { mode: "starting", aircraft: [], total: 0 } });
    render(<AdsbPanel r={r} />);
    expect(screen.getByText("Waiting for the decoder…")).toBeInTheDocument();
  });

  it("stops the instance and closes the view", async () => {
    const { r, adsb, pluginStore } = await setup();
    const stop = vi.spyOn(pluginStore, "stopInstance").mockResolvedValue();
    adsb.openView();
    render(<AdsbPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: /Stop ADS-B/ }));
    await flush();
    expect(stop).toHaveBeenCalledWith("i-adsb");
    expect(adsb.getSnapshot()).toMatchObject({ instanceId: null, viewOpen: false });
  });

  it("starts ADS-B only after the takeover is confirmed, with the receiver position", async () => {
    const { r, adsb, pluginStore } = await setup({ running: false });
    const start = vi
      .spyOn(pluginStore, "startInstance")
      .mockResolvedValue(instance({ id: "i-new", plugin_id: ADSB_PLUGIN_ID }));
    render(<AdsbPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: "Start ADS-B…" }));
    const dialog = await screen.findByRole("dialog", { name: "Start ADS-B on 1090 MHz?" });
    expect(dialog).toHaveTextContent(/takes the whole device/);
    expect(dialog).toHaveTextContent(/OpenStreetMap/);
    expect(start).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText("Latitude"), { target: { value: "51,25" } });
    fireEvent.change(within(dialog).getByLabelText("Longitude"), { target: { value: "7.5" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Start ADS-B" }));
    await flush();
    expect(start).toHaveBeenCalledWith({
      pluginId: ADSB_PLUGIN_ID,
      device: "dev0",
      config: { lat: 51.25, lon: 7.5 },
    });
    expect(adsb.getSnapshot()).toMatchObject({ instanceId: "i-new", viewOpen: true });
    // The position is remembered for the next start.
    expect(window.localStorage.getItem(RECEIVER_STORAGE_KEY)).toContain("51,25");
  });

  it("offers to stop DAB instead of failing with a claim message", async () => {
    const dab = instance({
      id: "i-dab",
      plugin_id: "org.superhet.decoder.dab",
      exclusive: true,
      state: "running",
    });
    const { r, pluginStore } = await setup({ running: false, instances: [dab] });
    const order: string[] = [];
    vi.spyOn(pluginStore, "stopInstance").mockImplementation(async (id) => void order.push(id));
    vi.spyOn(pluginStore, "startInstance").mockImplementation(async () => {
      order.push("start");
      return instance({ id: "i-new", plugin_id: ADSB_PLUGIN_ID });
    });
    render(<AdsbPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: "Start ADS-B…" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("DAB is using the device. Stop DAB to start ADS-B.");
    fireEvent.click(within(dialog).getByRole("button", { name: "Stop DAB and start ADS-B" }));
    await flush();
    expect(order).toEqual(["i-dab", "start"]);
  });

  it("starts without a position when the fields are empty", async () => {
    const { r, pluginStore } = await setup({ running: false });
    const start = vi
      .spyOn(pluginStore, "startInstance")
      .mockResolvedValue(instance({ id: "i-new", plugin_id: ADSB_PLUGIN_ID }));
    render(<AdsbPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: "Start ADS-B…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Start ADS-B" }));
    await flush();
    expect(start).toHaveBeenCalledWith({
      pluginId: ADSB_PLUGIN_ID,
      device: "dev0",
      config: undefined,
    });
  });

  it("refuses half a position or one off the globe", async () => {
    const { r, pluginStore } = await setup({ running: false });
    const start = vi.spyOn(pluginStore, "startInstance");
    render(<AdsbPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: "Start ADS-B…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Latitude"), { target: { value: "51" } });
    expect(dialog).toHaveTextContent(/both latitude and longitude/);
    fireEvent.change(within(dialog).getByLabelText("Longitude"), { target: { value: "700" } });
    expect(dialog).toHaveTextContent(/-180 to 180/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Start ADS-B" }));
    await flush();
    expect(start).not.toHaveBeenCalled();
  });

  it("shows why a start was refused", async () => {
    const { r, pluginStore } = await setup({ running: false });
    vi.spyOn(pluginStore, "startInstance").mockRejectedValue(new Error("the device is locked"));
    render(<AdsbPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: "Start ADS-B…" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Start ADS-B" }));
    await flush();
    expect(
      screen.getAllByRole("alert").some((a) => /device is locked/.test(a.textContent ?? "")),
    ).toBe(true);
  });

  it("explains a missing plugin", async () => {
    const { r } = await setup({ running: false, installed: false });
    render(<AdsbPanel r={r} />);
    expect(screen.getByText(/ADS-B plugin is not installed/)).toBeInTheDocument();
  });

  it("gives a listener the numbers and the map without any control", async () => {
    const { r } = await setup();
    render(asListener(<AdsbPanel r={r} />));
    expect(screen.getByTestId("adsb-summary")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open the aircraft map and table" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: /Stop ADS-B/ })).toBeNull();
  });

  it("tells a listener that ADS-B is not running", async () => {
    const { r } = await setup({ running: false });
    render(asListener(<AdsbPanel r={r} />));
    expect(screen.getByText("ADS-B is not running")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Start ADS-B…" })).toBeNull();
  });
});
