import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../spectrum/api";
import { vfo } from "../shell/fixtures";
import { toasts } from "../ui";
import { DecoderPicker } from "./DecoderPicker";
import { PluginStore } from "./store";
import {
  ADSB_ID,
  POCSAG_ID,
  RTL433_ID,
  adsb,
  fakePluginApi,
  instance,
  pocsag,
  rtl433,
} from "./testing";

beforeEach(() => {
  localStorage.clear();
  toasts.clear();
});

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

async function setup(
  opts: {
    plugins?: Parameters<typeof fakePluginApi>[0];
    instances?: Parameters<typeof fakePluginApi>[1];
    vfoOver?: Parameters<typeof vfo>[1];
    noVfo?: boolean;
  } = {},
) {
  const fake = fakePluginApi(opts.plugins ?? [pocsag, rtl433, adsb], opts.instances ?? []);
  const store = new PluginStore(fake.api);
  const onUpdateVfo = vi.fn();
  const view = render(
    <DecoderPicker
      store={store}
      device="dev0"
      vfo={opts.noVfo ? null : vfo(1, opts.vfoOver)}
      isAdmin
      onUpdateVfo={onUpdateVfo}
    />,
  );
  store.start();
  await flush();
  return { ...fake, store, onUpdateVfo, view };
}

describe("available decoders", () => {
  it("lists the decoders with the input they need and flags the ones that need settings", async () => {
    const { store } = await setup();
    const list = screen.getByRole("list", { name: "Available decoders" });
    expect(within(list).getByText("POCSAG / FLEX pagers")).toBeInTheDocument();
    expect(within(list).getByText("ISM sensors (rtl_433)")).toBeInTheDocument();
    // The 12.5 kHz NFM VFO fits POCSAG but not rtl_433.
    const rtl = screen.getByTestId(`plugin-${RTL433_ID}`);
    expect(rtl).toHaveTextContent("Needs settings");
    expect(screen.getByTestId(`plugin-${POCSAG_ID}`)).not.toHaveTextContent("Needs settings");
    // The exclusive plugin is listed apart.
    const exclusive = screen.getByRole("list", { name: "Exclusive decoders" });
    expect(exclusive).toHaveTextContent("Whole device");
    store.stop();
  });

  it("needs a receiver for shared plugins but not for exclusive ones", async () => {
    const { store } = await setup({ noVfo: true });
    expect(screen.getByRole("button", { name: "Start POCSAG / FLEX pagers" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Start ADS-B" })).toBeEnabled();
    store.stop();
  });

  it("says so when nothing is installed", async () => {
    const { store } = await setup({ plugins: [] });
    expect(screen.getByText("No decoder plugins installed")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "admin console" })).toBeInTheDocument();
    store.stop();
  });

  it("explains why the plugin list is missing", async () => {
    const { api, store } = await setup();
    api.listPlugins.mockRejectedValueOnce(new ApiError("plugin host is not running", 503));
    await act(() => store.refresh());
    expect(screen.getByRole("alert")).toHaveTextContent("plugin host is not running");
    store.stop();
  });
});

describe("starting a decoder", () => {
  it("shows the notice first, offers the VFO settings and starts with the config", async () => {
    const { api, store, onUpdateVfo } = await setup({
      vfoOver: { mode: "wfm", bandwidthHz: 180_000, squelchDbfs: -60 },
    });
    fireEvent.click(screen.getByRole("button", { name: "Start POCSAG / FLEX pagers" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Pager messages are private communications");
    const start = within(dialog).getByRole("button", { name: "Start" });
    // The first start needs the notice to be confirmed.
    expect(start).toBeDisabled();
    fireEvent.click(within(dialog).getByRole("switch", { name: /I have read this notice/ }));
    expect(start).toBeEnabled();

    // VFO suggestion with an Apply button.
    expect(dialog).toHaveTextContent("Suggested settings for VFO 1");
    fireEvent.click(within(dialog).getByRole("button", { name: "Apply to VFO 1" }));
    expect(onUpdateVfo).toHaveBeenCalledWith(1, {
      mode: "nfm",
      bandwidthHz: 12_500,
      squelchDbfs: null,
    });

    // Config form of the first-party plugin.
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "1200" }));
    fireEvent.change(within(dialog).getByRole("combobox", { name: "Signal polarity" }), {
      target: { value: "inverted" },
    });
    fireEvent.change(within(dialog).getByRole("combobox", { name: "National characters" }), {
      target: { value: "DE" },
    });
    fireEvent.click(start);
    await flush();

    expect(api.start).toHaveBeenCalledWith({
      pluginId: POCSAG_ID,
      device: "dev0",
      vfo: 1,
      freqHz: 100_100_000,
      config: { baud: [1200], invert: true, charset: "DE" },
    });
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(toasts.getSnapshot().some((t) => t.title === "POCSAG / FLEX pagers started")).toBe(true);
    // The running instance appears in the picker.
    expect(screen.getByRole("list", { name: "Running on this VFO" })).toHaveTextContent("Running");

    // The notice is remembered for the next start.
    fireEvent.click(screen.getByRole("button", { name: "Start POCSAG / FLEX pagers" }));
    expect(screen.getByRole("dialog")).toHaveTextContent("You confirmed this notice before");
    expect(within(screen.getByRole("dialog")).getByRole("button", { name: "Start" })).toBeEnabled();
    store.stop();
  });

  it("sends no config when the form is untouched and the VFO already fits", async () => {
    const { api, store } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "Start POCSAG / FLEX pagers" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).not.toHaveTextContent("Suggested settings");
    fireEvent.click(within(dialog).getByRole("switch", { name: /I have read this notice/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Start" }));
    await flush();
    expect(api.start.mock.calls[0]?.[0]).toMatchObject({ pluginId: POCSAG_ID, config: {} });
    store.stop();
  });

  it("keeps the dialog open and shows the server's reason when the start is refused", async () => {
    const { api, store } = await setup();
    api.start.mockRejectedValueOnce(new ApiError("unknown config key: bogus", 400, "invalid"));
    fireEvent.click(screen.getByRole("button", { name: "Start POCSAG / FLEX pagers" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("switch", { name: /I have read this notice/ }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Start" }));
    await flush();
    expect(screen.getByRole("dialog")).toHaveTextContent("unknown config key: bogus");
    expect(within(screen.getByRole("dialog")).getByRole("button", { name: "Start" })).toBeEnabled();
    store.stop();
  });

  it("takes a JSON config for plugins without a form", async () => {
    const other = {
      ...adsb,
      id: "org.example.decoder.other",
      name: "Other",
      device: { ...adsb.device, exclusive: false },
      acknowledge: null,
    };
    const { api, store } = await setup({ plugins: [other] });
    fireEvent.click(screen.getByRole("button", { name: "Start Other" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/Config \(JSON/), { target: { value: "[1]" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Start" }));
    await flush();
    expect(dialog).toHaveTextContent("must be a JSON object");
    expect(api.start).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText(/Config \(JSON/), {
      target: { value: '{"a":1}' },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Start" }));
    await flush();
    expect(api.start.mock.calls[0]?.[0]).toMatchObject({ config: { a: 1 } });
    store.stop();
  });

  it("asks for confirmation before an exclusive plugin takes the device over", async () => {
    const { api, store } = await setup();
    fireEvent.click(screen.getByRole("button", { name: "Start ADS-B" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("This decoder needs the whole device");
    expect(dialog).toHaveTextContent("dev0 is tuned to 1090.000 MHz");
    const go = within(dialog).getByRole("button", { name: "Take over and start" });
    expect(go).toBeDisabled();
    fireEvent.click(within(dialog).getByRole("switch", { name: /take over the device/ }));
    fireEvent.click(go);
    await flush();
    // No VFO: the plugin gets the window. A fixed centre is not sent.
    expect(api.start.mock.calls[0]?.[0]).toEqual({ pluginId: ADSB_ID, device: "dev0", config: {} });
    store.stop();
  });
});

describe("running instances", () => {
  it("shows state, restarts and counters and stops an instance", async () => {
    const { api, store } = await setup({
      instances: [instance({ restarts: 2, events_ok: 5, input_dropped: 7 })],
    });
    const row = screen.getByTestId("instance-i-1");
    expect(row).toHaveTextContent("Running");
    expect(row).toHaveTextContent("VFO 1 · dev0");
    expect(within(row).getByText("Restarts")).toBeInTheDocument();
    expect(within(row).getByTestId("facts-decoded")).toHaveTextContent("5 events");
    expect(row).toHaveTextContent("7 chunks");
    // Liveness: what arrived and when the last event was.
    expect(within(row).getByTestId("facts-input")).toHaveTextContent("441 kB");
    expect(within(row).getByTestId("facts-input")).toHaveTextContent("-33 dBFS");
    fireEvent.click(within(row).getByRole("button", { name: "Stop POCSAG / FLEX pagers" }));
    await flush();
    expect(api.stop).toHaveBeenCalledWith("i-1");
    expect(screen.queryByTestId("instance-i-1")).toBeNull();
    store.stop();
  });

  it("shows the last error of a failed instance and restarts it", async () => {
    const { api, store } = await setup({
      instances: [instance({ state: "error", last_error: "exit status 3", restarts: 5 })],
    });
    const row = screen.getByTestId("instance-i-1");
    expect(row).toHaveTextContent("Failed");
    expect(row).toHaveTextContent("exit status 3");
    fireEvent.click(within(row).getByRole("button", { name: "Restart" }));
    await flush();
    expect(api.restart).toHaveBeenCalledWith("i-1");
    store.stop();
  });

  it("reports a refused stop as a toast", async () => {
    const { api, store } = await setup({ instances: [instance()] });
    api.stop.mockRejectedValueOnce(new ApiError("not found", 404, "not_found"));
    fireEvent.click(screen.getByRole("button", { name: "Stop POCSAG / FLEX pagers" }));
    await flush();
    expect(toasts.getSnapshot().some((t) => t.title.startsWith("Could not stop"))).toBe(true);
    store.stop();
  });

  it("lists exclusive instances under the device", async () => {
    const { store } = await setup({
      instances: [instance({ id: "i-9", plugin_id: ADSB_ID, vfo: null, exclusive: true })],
    });
    expect(screen.getByRole("list", { name: "Running on the device" })).toHaveTextContent(
      "whole device",
    );
    store.stop();
  });
});
