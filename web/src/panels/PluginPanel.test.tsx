import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DecoderEvent } from "../decoders/events";
import type { PluginInstance } from "../plugins/api";
import { panelUrl, parsePanel, type PanelInfo } from "./api";
import { PluginPanel } from "./PluginPanel";

const panel: PanelInfo = {
  pluginId: "org.superhet.example.echo",
  name: "Example echo",
  version: "1.0.0",
  title: "Echo view",
  placement: "inspector",
  entry: "index.html",
  reads: ["org.superhet.example.echo"],
  tune: true,
};

const echoEvent: DecoderEvent = {
  id: 7,
  tsMs: 7000,
  device: "dev0",
  vfo: null,
  freqHz: null,
  mode: null,
  kind: "echo.block",
  source: "org.superhet.example.echo",
  value: "block 1",
  meta: {},
};
const otherEvent: DecoderEvent = { ...echoEvent, id: 8, source: "core", value: "other" };

function setup(props: { isOperator: boolean; tune?: boolean }) {
  const onTune = vi.fn();
  const view = render(
    <PluginPanel
      panel={{ ...panel, tune: props.tune ?? true }}
      instances={[] as PluginInstance[]}
      events={[otherEvent, echoEvent]}
      isOperator={props.isOperator}
      onTune={onTune}
    />,
  );
  const frame = screen.getByTitle("Echo view") as HTMLIFrameElement;
  const win = frame.contentWindow as Window;
  const sent: Record<string, unknown>[] = [];
  vi.spyOn(win, "postMessage").mockImplementation((message: unknown) => {
    sent.push(message as Record<string, unknown>);
  });
  /** Deliver a message to the app as the browser would. */
  const deliver = (data: unknown, over: { source?: Window | null; origin?: string } = {}) =>
    act(() => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data,
          origin: over.origin ?? "null",
          source: "source" in over ? over.source : win,
        }),
      );
    });
  return { onTune, frame, win, sent, deliver, ...view };
}

beforeEach(() => vi.restoreAllMocks());

describe("the panel frame", () => {
  it("is sandboxed: scripts only, no same-origin, no navigation, no powers", () => {
    const { frame } = setup({ isOperator: true });
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("allow")).toBe("");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
    const sandbox = frame.getAttribute("sandbox") ?? "";
    for (const power of [
      "allow-same-origin",
      "allow-top-navigation",
      "allow-popups",
      "allow-forms",
      "allow-modals",
      "allow-downloads",
    ]) {
      expect(sandbox).not.toContain(power);
    }
    expect(frame.getAttribute("src")).toBe(
      "/api/v1/plugin-ui/org.superhet.example.echo/index.html?v=1.0.0",
    );
  });

  it("shows the example panel: init after hello, then instances and only its own events", () => {
    const { sent, deliver } = setup({ isOperator: true });
    expect(sent).toEqual([]); // nothing is pushed before the panel asks
    deliver({ superhet: 1, type: "hello" });
    const init = sent.find((m) => m.type === "init");
    expect(init).toMatchObject({
      superhet: 1,
      type: "init",
      plugin_id: "org.superhet.example.echo",
      can_tune: true,
    });
    expect(init?.theme).toBeTypeOf("object");
    const events = sent.find((m) => m.type === "events") as { events: { value: string }[] };
    expect(events.events.map((e) => e.value)).toEqual(["block 1"]);
    expect(sent.some((m) => m.type === "instances")).toBe(true);
  });

  it("ignores messages from another window, an origin or a malformed shape", () => {
    const { sent, deliver, onTune } = setup({ isOperator: true });
    deliver({ superhet: 1, type: "hello" }, { source: window });
    deliver({ superhet: 1, type: "hello" }, { source: null });
    deliver({ superhet: 1, type: "hello" }, { origin: "https://evil.example" });
    deliver({ superhet: 1, type: "tune", freq_hz: 1e8 }, { source: window });
    deliver({ superhet: 1, type: "hello", extra: 1 });
    deliver("hello");
    expect(sent).toEqual([]);
    expect(onTune).not.toHaveBeenCalled();
  });

  it("tunes for an operator, and only once per interval", () => {
    const { sent, deliver, onTune } = setup({ isOperator: true });
    deliver({ superhet: 1, type: "hello" });
    deliver({ superhet: 1, type: "tune", freq_hz: 100_000_000 });
    expect(onTune).toHaveBeenCalledWith(100_000_000);
    expect(sent.at(-1)).toMatchObject({ type: "tune_result", ok: true });
    deliver({ superhet: 1, type: "tune", freq_hz: 101_000_000 });
    expect(onTune).toHaveBeenCalledTimes(1);
    expect(sent.at(-1)).toMatchObject({ type: "tune_result", ok: false });
  });

  it("refuses tuning for listeners and guests and tells the panel", () => {
    const { sent, deliver, onTune } = setup({ isOperator: false });
    deliver({ superhet: 1, type: "hello" });
    expect(sent.find((m) => m.type === "init")).toMatchObject({ can_tune: false });
    deliver({ superhet: 1, type: "tune", freq_hz: 100_000_000 });
    expect(onTune).not.toHaveBeenCalled();
    expect(sent.at(-1)).toMatchObject({ type: "tune_result", ok: false, error: "not allowed" });
  });

  it("refuses tuning for a panel whose manifest did not ask for it, operator or not", () => {
    const { sent, deliver, onTune } = setup({ isOperator: true, tune: false });
    deliver({ superhet: 1, type: "hello" });
    expect(sent.find((m) => m.type === "init")).toMatchObject({ can_tune: false });
    deliver({ superhet: 1, type: "tune", freq_hz: 100_000_000 });
    expect(onTune).not.toHaveBeenCalled();
  });

  it("closes a frame that navigates away", () => {
    const { frame } = setup({ isOperator: true });
    fireEvent.load(frame); // the page itself
    expect(screen.getByTitle("Echo view")).toBeInTheDocument();
    fireEvent.load(frame); // it navigated its own frame
    expect(screen.queryByTitle("Echo view")).toBeNull();
    expect(screen.getByRole("alert")).toHaveTextContent("navigated away");
  });
});

describe("panel list entries", () => {
  const raw = {
    plugin_id: "org.superhet.example.echo",
    name: "Example echo",
    version: "1.0.0",
    title: "Echo view",
    placement: "inspector",
    entry: "index.html",
    reads: ["org.superhet.example.echo"],
    tune: true,
  };

  it("parses a good entry and falls back to the plugin itself for reads", () => {
    expect(parsePanel(raw)).toEqual(panel);
    expect(parsePanel({ ...raw, reads: [] })?.reads).toEqual([raw.plugin_id]);
    expect(parsePanel({ ...raw, tune: "yes" })?.tune).toBe(false);
  });

  it("drops entries that could build a path outside the plugin's panel", () => {
    for (const bad of [
      { ...raw, entry: "../x.html" },
      { ...raw, entry: "a/b.html" },
      { ...raw, entry: ".hidden" },
      { ...raw, entry: "" },
      { ...raw, plugin_id: "../etc" },
      { ...raw, plugin_id: "a.b" },
      { ...raw, placement: "dock" },
      { ...raw, title: "" },
      { ...raw, reads: "x" },
      null,
      "x",
    ]) {
      expect(parsePanel(bad), JSON.stringify(bad)).toBeNull();
    }
  });

  it("builds the URL from encoded parts", () => {
    expect(panelUrl(panel)).toBe("/api/v1/plugin-ui/org.superhet.example.echo/index.html?v=1.0.0");
    expect(panelUrl({ ...panel, version: "1.0.0&x=1" })).toContain("?v=1.0.0%26x%3D1");
  });
});
