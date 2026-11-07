import { act, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { DecoderEventStore } from "../decoders/store";
import { PluginStore } from "../plugins/store";
import { fake } from "../shell/fixtures";
import { visibleTabs } from "../shell/inspector";
import type { PanelApi, PanelInfo } from "./api";
import { PanelsTab } from "./PanelsTab";
import { PANEL_POLL_MS, PanelStore } from "./store";

const echo: PanelInfo = {
  pluginId: "org.superhet.example.echo",
  name: "Example echo",
  version: "1.0.0",
  title: "Echo view",
  placement: "inspector",
  entry: "index.html",
  reads: ["org.superhet.example.echo"],
  tune: true,
};

function receiver(panels: PanelInfo[]) {
  const { r } = fake([], null);
  const api: PanelApi = { list: vi.fn().mockResolvedValue(panels) };
  Object.assign(r, {
    panels: new PanelStore(api),
    decoderEvents: new DecoderEventStore({ device: "dev0" }),
    plugins: new PluginStore({} as never),
  });
  return { r, api };
}

describe("the plugins tab", () => {
  it("shows the example panel in a sandboxed frame once the server lists it", async () => {
    const { r } = receiver([echo]);
    await act(async () => {
      r.panels.start();
      await r.panels.refresh();
    });
    render(<PanelsTab r={r} />);
    const frame = screen.getByTitle("Echo view");
    expect(frame.tagName).toBe("IFRAME");
    expect(frame).toHaveAttribute("sandbox", "allow-scripts");
    expect(frame).toHaveAttribute(
      "src",
      "/api/v1/plugin-ui/org.superhet.example.echo/index.html?v=1.0.0",
    );
    r.panels.stop();
  });

  it("offers a picker when several plugins ship a panel", async () => {
    const { r } = receiver([echo, { ...echo, pluginId: "org.superhet.ui.other", title: "Other" }]);
    await act(async () => {
      r.panels.start();
      await r.panels.refresh();
    });
    render(<PanelsTab r={r} />);
    expect(screen.getByRole("combobox", { name: "Plugin panel" })).toBeInTheDocument();
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["Echo view", "Other"]);
    r.panels.stop();
  });

  it("says so when no plugin ships a panel", () => {
    const { r } = receiver([]);
    render(<PanelsTab r={r} />);
    expect(screen.getByText("No plugin panels")).toBeInTheDocument();
  });
});

describe("the panel store", () => {
  it("polls, keeps the last list on a failure and stops with the receiver", async () => {
    vi.useFakeTimers();
    const list = vi.fn().mockResolvedValueOnce([echo]).mockRejectedValue(new Error("down"));
    const store = new PanelStore({ list });
    store.start();
    await vi.advanceTimersByTimeAsync(0);
    expect(store.getSnapshot()).toEqual({ panels: [echo], loaded: true });
    await vi.advanceTimersByTimeAsync(PANEL_POLL_MS);
    expect(list).toHaveBeenCalledTimes(2);
    expect(store.getSnapshot().panels).toEqual([echo]);
    store.stop();
    await vi.advanceTimersByTimeAsync(PANEL_POLL_MS * 2);
    expect(list).toHaveBeenCalledTimes(2);
    vi.useRealTimers();
  });

  it("starts empty-but-loaded when the first request fails", async () => {
    const store = new PanelStore({ list: vi.fn().mockRejectedValue(new Error("down")) });
    store.start();
    await store.refresh();
    expect(store.getSnapshot()).toEqual({ panels: [], loaded: true });
    store.stop();
  });
});

describe("the tab list", () => {
  const perms = { canReadBookmarks: true, canSeeDecoders: true };
  it("has the plugins tab only while a plugin ships a panel and decoder output is visible", () => {
    expect(visibleTabs(perms)).not.toContain("panels");
    expect(visibleTabs(perms, true)).toContain("panels");
    expect(visibleTabs({ ...perms, canSeeDecoders: false }, true)).not.toContain("panels");
  });
});
