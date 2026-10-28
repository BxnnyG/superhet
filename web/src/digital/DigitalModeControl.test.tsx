import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
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
import { DigitalModeControl } from "./DigitalModeControl";

beforeEach(() => {
  localStorage.clear();
  toasts.clear();
});

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

async function setup(
  opts: {
    vfoOver?: Parameters<typeof vfo>[1];
    instances?: Parameters<typeof fakePluginApi>[1];
    plugins?: Parameters<typeof fakePluginApi>[0];
  } = {},
) {
  const fake = fakePluginApi(opts.plugins ?? [pocsag, aprs, rtl433], opts.instances ?? []);
  const store = new PluginStore(fake.api);
  const onUpdate = vi.fn();
  const v = vfo(1, { mode: "wfm", bandwidthHz: 180_000, squelchDbfs: -60, ...opts.vfoOver });
  const view = render(
    <DigitalModeControl vfo={v} device="dev0" plugins={store} onUpdate={onUpdate} />,
  );
  store.start();
  await flush();
  return { ...fake, store, onUpdate, view };
}

const select = () => screen.getByRole("combobox", { name: "Digital mode" });

it("offers the installed digital modes and Analog only while one runs", async () => {
  const { store } = await setup();
  expect(
    within(select())
      .getAllByRole("option")
      .map((o) => o.textContent),
  ).toEqual(["Digital…", "POCSAG", "APRS", "ISM"]);
  store.stop();
});

it("is not shown when no digital decoder is installed", async () => {
  const { store } = await setup({ plugins: [] });
  expect(screen.queryByRole("combobox", { name: "Digital mode" })).toBeNull();
  store.stop();
});

it("sets the receiver up and starts the plugin after the first-time notice", async () => {
  const { api, onUpdate, store } = await setup();
  fireEvent.change(select(), { target: { value: "pocsag" } });
  await flush();
  // The legal notice comes first; nothing was changed yet.
  const dialog = screen.getByRole("dialog", { name: "Start POCSAG" });
  expect(api.start).not.toHaveBeenCalled();
  expect(onUpdate).not.toHaveBeenCalled();
  const go = within(dialog).getByRole("button", { name: "Start POCSAG" });
  expect(go).toBeDisabled();
  fireEvent.click(within(dialog).getByRole("switch"));
  fireEvent.click(go);
  await flush();
  expect(onUpdate).toHaveBeenCalledWith(1, { mode: "nfm", bandwidthHz: 12_500, squelchDbfs: null });
  expect(api.start).toHaveBeenCalledWith({
    pluginId: POCSAG_ID,
    device: "dev0",
    vfo: 1,
    freqHz: 100_100_000,
  });
  expect(screen.queryByRole("dialog")).toBeNull();
  // The mode shows as the selection and Analog is the way back.
  expect(select()).toHaveValue("pocsag");
  expect(within(select()).getAllByRole("option")[0]?.textContent).toBe("Analog");
  store.stop();
});

it("switching modes stops the old decoder and uses the ISM width", async () => {
  const { api, onUpdate, store } = await setup();
  fireEvent.change(select(), { target: { value: "pocsag" } });
  await flush();
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("switch"));
  fireEvent.click(screen.getByRole("button", { name: "Start POCSAG" }));
  await flush();
  api.start.mockClear();
  onUpdate.mockClear();
  // Switching to ISM stops POCSAG on this receiver and starts rtl_433 with the wide preset.
  fireEvent.change(select(), { target: { value: "ism" } });
  await flush();
  expect(screen.getByRole("dialog", { name: "Start ISM" })).toBeInTheDocument();
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("switch"));
  fireEvent.click(screen.getByRole("button", { name: "Start ISM" }));
  await flush();
  expect(api.stop).toHaveBeenCalledWith("i-1");
  expect(onUpdate).toHaveBeenCalledWith(1, {
    mode: "nfm",
    bandwidthHz: 250_000,
    squelchDbfs: null,
  });
  expect(api.start).toHaveBeenCalledWith(expect.objectContaining({ pluginId: RTL433_ID, vfo: 1 }));
  store.stop();
});

it("stops the decoder with Analog", async () => {
  const { api, store } = await setup({ instances: [instance({ plugin_id: APRS_ID })] });
  expect(select()).toHaveValue("aprs");
  fireEvent.change(select(), { target: { value: "" } });
  await flush();
  expect(api.stop).toHaveBeenCalledWith("i-1");
  store.stop();
});

it("suggests the mode that usually lives on the frequency", async () => {
  const { api, store } = await setup({ vfoOver: { freqHz: 144_800_000 } });
  const chip = screen.getByRole("button", { name: "Try APRS here" });
  expect(chip).toHaveAttribute("title", expect.stringContaining("APRS Europe"));
  // Confirm the notice once, then the chip starts it.
  fireEvent.click(chip);
  await flush();
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("switch"));
  fireEvent.click(screen.getByRole("button", { name: "Start APRS" }));
  await flush();
  expect(api.start).toHaveBeenCalledWith(expect.objectContaining({ pluginId: APRS_ID }));
  // Nothing to suggest once a mode runs.
  expect(screen.queryByRole("button", { name: /^Try / })).toBeNull();
  store.stop();
});

it("has no suggestion on an unrelated frequency", async () => {
  const { store } = await setup({ vfoOver: { freqHz: 99_200_000 } });
  expect(screen.queryByRole("button", { name: /^Try / })).toBeNull();
  store.stop();
});

it("reports a refused start", async () => {
  const { api, store } = await setup();
  api.start.mockRejectedValueOnce(new Error("invalid config"));
  fireEvent.change(select(), { target: { value: "aprs" } });
  await flush();
  fireEvent.click(within(screen.getByRole("dialog")).getByRole("switch"));
  fireEvent.click(screen.getByRole("button", { name: "Start APRS" }));
  await flush();
  expect(toasts.getSnapshot().some((t) => t.title === "Could not start APRS")).toBe(true);
  store.stop();
});
