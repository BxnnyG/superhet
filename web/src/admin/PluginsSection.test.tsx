import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ApiError } from "../spectrum/api";
import { toasts } from "../ui";
import { ADSB_ID, POCSAG_ID, fakePluginApi, instance, pocsag, rtl433 } from "../plugins/testing";
import { adsb } from "../plugins/testing";
import { PluginsSection } from "./PluginsSection";

beforeEach(() => toasts.clear());

const flush = () => act(() => new Promise<void>((r) => setTimeout(r, 0)));

async function mount(instances = [instance()]) {
  const fake = fakePluginApi([pocsag, rtl433, adsb], instances);
  render(<PluginsSection api={fake.api} />);
  await screen.findByRole("list", { name: "Installed plugins" });
  return fake;
}

describe("installed plugins", () => {
  it("shows name, version, kind, licence, the bundled badge and the notice", async () => {
    await mount();
    const row = screen.getByTestId(`plugin-${POCSAG_ID}`);
    expect(row).toHaveTextContent("POCSAG / FLEX pagers");
    expect(row).toHaveTextContent("v0.1.0");
    expect(row).toHaveTextContent("decoder");
    expect(row).toHaveTextContent("AGPL-3.0-only");
    expect(row).toHaveTextContent("Bundled");
    expect(row).toHaveTextContent("1 running");
    expect(row).toHaveTextContent("Install notice");
    expect(row).toHaveTextContent("Pager messages are private communications");
    // Third-party plugins are not bundled; exclusive ones say so.
    const other = screen.getByTestId(`plugin-${ADSB_ID}`);
    expect(other).not.toHaveTextContent("Bundled");
    expect(other).toHaveTextContent("Whole device");
    expect(other).toHaveTextContent("claims 1090.000 MHz");
  });

  it("removes a plugin after a confirmation", async () => {
    const { api } = await mount([]);
    fireEvent.click(screen.getByRole("button", { name: `Remove ADS-B` }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Remove ADS-B?");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await flush();
    expect(api.remove).toHaveBeenCalledWith(ADSB_ID);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("shows the server's reason when a plugin cannot be removed", async () => {
    const { api } = await mount();
    api.remove.mockRejectedValueOnce(
      new ApiError("an instance of the plugin exists", 409, "conflict"),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove POCSAG / FLEX pagers" }));
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
    await flush();
    expect(toasts.getSnapshot().some((t) => t.body === "an instance of the plugin exists")).toBe(
      true,
    );
  });
});

describe("install from a path", () => {
  it("installs and then shows the notice", async () => {
    const { api } = await mount();
    fireEvent.click(screen.getByRole("button", { name: "Install from path" }));
    const dialog = screen.getByRole("dialog");
    const install = within(dialog).getByRole("button", { name: "Install" });
    expect(install).toBeDisabled();
    fireEvent.change(within(dialog).getByLabelText(/Directory with plugin.toml/), {
      target: { value: " /plugins-src/pocsag " },
    });
    fireEvent.click(install);
    await flush();
    expect(api.install).toHaveBeenCalledWith("/plugins-src/pocsag");
    expect(await screen.findByText("POCSAG / FLEX pagers installed")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toHaveTextContent("Pager messages are private");
  });

  it("keeps the dialog open on a conflict", async () => {
    const { api } = await mount();
    api.install.mockRejectedValueOnce(new ApiError("already installed", 409, "conflict"));
    fireEvent.click(screen.getByRole("button", { name: "Install from path" }));
    const dialog = screen.getByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText(/Directory with plugin.toml/), {
      target: { value: "/x" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Install" }));
    await flush();
    expect(screen.getByRole("alert")).toHaveTextContent("already installed");
  });
});

describe("instances", () => {
  it("lists state, binding, restarts, drops and the last error", async () => {
    await mount([
      instance({ id: "i-1", restarts: 4, crashes_in_window: 2, input_dropped: 12 }),
      instance({
        id: "i-2",
        state: "error",
        last_error: "exit status 3",
        vfo: null,
        device: "dev0",
        exclusive: true,
      }),
    ]);
    const table = screen.getByRole("table", { name: "Plugin instances" });
    const ok = within(table).getByTestId("instance-i-1");
    expect(ok).toHaveTextContent("Running");
    expect(ok).toHaveTextContent("VFO 1 · dev0");
    expect(ok).toHaveTextContent("4");
    expect(ok).toHaveTextContent("2 recent");
    expect(ok).toHaveTextContent("12");
    const failed = within(table).getByTestId("instance-i-2");
    expect(failed).toHaveTextContent("Failed");
    expect(failed).toHaveTextContent("whole device");
    expect(failed).toHaveTextContent("exit status 3");
  });

  it("stops and restarts instances", async () => {
    const { api } = await mount([
      instance({ id: "i-1" }),
      instance({ id: "i-2", state: "error", last_error: "boom" }),
    ]);
    fireEvent.click(
      within(screen.getByTestId("instance-i-2")).getByRole("button", { name: "Restart" }),
    );
    await flush();
    expect(api.restart).toHaveBeenCalledWith("i-2");
    fireEvent.click(
      within(screen.getByTestId("instance-i-1")).getByRole("button", { name: /^Stop / }),
    );
    await flush();
    expect(api.stop).toHaveBeenCalledWith("i-1");
    await waitFor(() => expect(screen.queryByTestId("instance-i-1")).toBeNull());
  });

  it("explains an empty instance list", async () => {
    await mount([]);
    expect(await screen.findByText("Nothing is running")).toBeInTheDocument();
  });
});
