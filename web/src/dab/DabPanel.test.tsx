import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AuthContext, type AuthContextValue } from "../auth/context";
import { FULL_ACCESS } from "../auth/permissions";
import { PluginStore } from "../plugins/store";
import { fakePluginApi, instance } from "../plugins/testing";
import { fake, vfo } from "../shell/fixtures";
import type { Receiver } from "../shell/useReceiver";
import { DabPanel } from "./DabPanel";
import { DabStore } from "./store";
import { dabReply, dabStatusRaw, fakeDabApi, fakeEventApi } from "./testing";

beforeEach(() => vi.clearAllMocks());

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

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

async function setup(opts: { running?: boolean; status?: Record<string, unknown> } = {}) {
  const { r } = fake([vfo(1)], 1);
  const dabApi = fakeDabApi(dabReply(opts.status ? { status: dabStatusRaw(opts.status) } : {}));
  const dab = new DabStore("dev0", dabApi.api, fakeEventApi());
  const plugins = fakePluginApi(undefined, []);
  const pluginStore = new PluginStore(plugins.api);
  const enable = vi.fn();
  Object.assign(r, {
    dab,
    plugins: pluginStore,
    audioSnap: {
      enabled: false,
      levels: { 1073741900: { volume: 1, muted: false, listening: true } },
      master: 1,
    },
    audio: { enable, setListening: vi.fn() },
  });
  dab.start();
  pluginStore.start();
  if (opts.running !== false) dab.follow("i-dab");
  await flush();
  return { r: r as Receiver, dab, dabApi, plugins, pluginStore, enable };
}

describe("DabPanel", () => {
  it("shows the ensemble, the services and what plays, with the slide", async () => {
    const { r } = await setup();
    render(<DabPanel r={r} />);
    expect(screen.getByTestId("dab-ensemble")).toHaveTextContent("DR Deutschland");
    const services = screen.getByRole("list", { name: "Services" });
    expect(within(services).getByText("Deutschlandfunk")).toBeInTheDocument();
    expect(within(services).getByText(/Culture · DAB · 128 kbit\/s/)).toBeInTheDocument();
    expect(screen.getByTestId("dab-dls")).toHaveTextContent("Nachrichten");
    const slide = screen.getByRole("img", { name: /Slideshow image of Deutschlandfunk/ });
    expect(slide).toHaveAttribute("src", "/api/v1/plugin-instances/i-dab/media/slide.d210?rev=2");
    expect(screen.getByText(/Good signal/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop Deutschlandfunk" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("an operator plays another service, tunes a found ensemble, scans and stops", async () => {
    const { r, dabApi, pluginStore } = await setup({
      status: {
        last_scan: {
          finished_unix_ms: 1,
          found: [{ channel: "6A", freq_hz: 181936000, label: "SWR BW", eid: "D3", services: 6 }],
        },
      },
    });
    const stopSpy = vi.spyOn(pluginStore, "stopInstance").mockResolvedValue(undefined);
    render(<DabPanel r={r} />);

    fireEvent.click(screen.getByRole("button", { name: "Play DKultur" }));
    await flush();
    expect(dabApi.api.control).toHaveBeenCalledWith("i-dab", { cmd: "select", service: "D212" });

    fireEvent.click(screen.getByRole("button", { name: /^6A/ }));
    await flush();
    expect(dabApi.api.control).toHaveBeenCalledWith("i-dab", { cmd: "tune", channel: "6A" });

    fireEvent.click(screen.getByRole("button", { name: "Scan all blocks" }));
    await flush();
    expect(dabApi.api.control).toHaveBeenCalledWith("i-dab", { cmd: "scan" });

    fireEvent.click(screen.getByRole("button", { name: /Stop DAB/ }));
    await flush();
    expect(stopSpy).toHaveBeenCalledWith("i-dab");
  });

  it("shows scan progress with the ensembles found so far", async () => {
    const { r, dabApi } = await setup({
      status: {
        mode: "scanning",
        scan: {
          index: 4,
          total: 38,
          channel: "6B",
          found: [
            {
              channel: "5C",
              freq_hz: 178352000,
              label: "DR Deutschland",
              eid: "D210",
              services: 8,
            },
          ],
        },
      },
    });
    render(<DabPanel r={r} />);
    expect(screen.getByText("Scanning 6B")).toBeInTheDocument();
    expect(screen.getByRole("progressbar", { name: "Scan progress" })).toHaveAttribute(
      "aria-valuenow",
      "11",
    );
    expect(screen.getByRole("list", { name: "Found so far" })).toHaveTextContent("DR Deutschland");
    fireEvent.click(screen.getByRole("button", { name: "Stop scan" }));
    await flush();
    expect(dabApi.api.control).toHaveBeenCalledWith("i-dab", { cmd: "scan_stop" });
  });

  it("starts DAB only after the takeover is confirmed", async () => {
    const { r, dab, pluginStore } = await setup({ running: false });
    const start = vi
      .spyOn(pluginStore, "startInstance")
      .mockResolvedValue(instance({ id: "i-new", pluginId: "org.superhet.decoder.dab" }));
    render(<DabPanel r={r} />);
    dab.request({ channel: "5C", freqHz: 178_352_000 });
    const dialog = await screen.findByRole("dialog", { name: "Start DAB on 5C?" });
    expect(dialog).toHaveTextContent(/takes the whole device/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Start DAB" }));
    await flush();
    expect(start).toHaveBeenCalledWith({
      pluginId: "org.superhet.decoder.dab",
      device: "dev0",
      freqHz: 178_352_000,
    });
    expect(dab.getSnapshot()).toMatchObject({ instanceId: "i-new", pending: null });
  });

  it("explains a missing DAB plugin", async () => {
    const { r } = await setup({ running: false });
    render(<DabPanel r={r} />);
    expect(screen.getByText(/DAB plugin is not installed/)).toBeInTheDocument();
  });

  it("shows why a start was refused", async () => {
    const { r, dab, pluginStore } = await setup({ running: false });
    vi.spyOn(pluginStore, "startInstance").mockRejectedValue(new Error("the device is locked"));
    render(<DabPanel r={r} />);
    dab.request({ channel: "5C", freqHz: 178_352_000 });
    const dialog = await screen.findByRole("dialog", { name: "Start DAB on 5C?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Start DAB" }));
    await flush();
    expect(
      screen.getAllByRole("alert").some((a) => /device is locked/.test(a.textContent ?? "")),
    ).toBe(true);
    expect(dab.getSnapshot().instanceId).toBeNull();
  });

  it("gives a listener the same view without any control", async () => {
    const { r } = await setup();
    render(asListener(<DabPanel r={r} />));
    expect(screen.getByTestId("dab-ensemble")).toHaveTextContent("DR Deutschland");
    expect(screen.getByText("Playing")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Play / })).toBeNull();
    expect(screen.queryByRole("button", { name: /Stop DAB/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Scan all blocks" })).toBeNull();
  });

  it("tells a listener that DAB is not running", async () => {
    const { r } = await setup({ running: false });
    render(asListener(<DabPanel r={r} />));
    expect(screen.getByText("DAB is not running")).toBeInTheDocument();
  });

  it("offers to enable audio, which needs a click", async () => {
    const { r, enable } = await setup();
    render(<DabPanel r={r} />);
    fireEvent.click(screen.getByRole("button", { name: "Enable audio" }));
    expect(enable).toHaveBeenCalled();
  });
});
