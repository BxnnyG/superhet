import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { APPLY_DELAY_MS, InstanceSettings } from "./InstanceSettings";
import { PluginStore } from "./store";
import { APRS_ID, aprs, fakePluginApi, instance, pocsag } from "./testing";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

function setup(over: Record<string, unknown> = {}) {
  const inst = instance(over);
  const fake = fakePluginApi([pocsag, aprs], [inst]);
  const store = new PluginStore(fake.api);
  render(<InstanceSettings instance={inst} store={store} />);
  return { ...fake, store, inst };
}

const settle = () => act(() => vi.advanceTimersByTimeAsync(APPLY_DELAY_MS + 50));

it("applies a change live after a short pause and says so", async () => {
  const { api } = setup();
  fireEvent.change(screen.getByRole("combobox", { name: "Signal polarity" }), {
    target: { value: "inverted" },
  });
  expect(screen.getByRole("status")).toHaveTextContent("Applying in a moment");
  expect(api.reconfigure).not.toHaveBeenCalled();
  await settle();
  expect(api.reconfigure).toHaveBeenCalledWith("i-1", { invert: true });
  expect(screen.getByRole("status")).toHaveTextContent("Applied");
});

it("sends one request for a burst of edits, with all of them", async () => {
  const { api } = setup();
  fireEvent.click(screen.getByRole("checkbox", { name: "1200" }));
  fireEvent.click(screen.getByRole("switch", { name: "Decode FLEX as well" }));
  await act(() => vi.advanceTimersByTimeAsync(APPLY_DELAY_MS - 100));
  fireEvent.change(screen.getByRole("combobox", { name: "National characters" }), {
    target: { value: "DE" },
  });
  await settle();
  expect(api.reconfigure).toHaveBeenCalledTimes(1);
  expect(api.reconfigure).toHaveBeenCalledWith("i-1", {
    baud: [1200],
    flex: false,
    charset: "DE",
  });
});

it("starts from the running config", () => {
  setup({ config: { baud: [512], invert: false } });
  expect(screen.getByRole("checkbox", { name: "512" })).toBeChecked();
  expect(screen.getByRole("checkbox", { name: "1200" })).not.toBeChecked();
  expect(screen.getByRole("combobox", { name: "Signal polarity" })).toHaveValue("normal");
});

it("shows the server's refusal", async () => {
  const { api } = setup();
  api.reconfigure.mockRejectedValueOnce(new Error("no such instance"));
  fireEvent.change(screen.getByRole("combobox", { name: "Signal polarity" }), {
    target: { value: "normal" },
  });
  await settle();
  expect(screen.getByRole("status")).toHaveTextContent("no such instance");
});

it("rejects a bad protocol list on the client", async () => {
  const fake = fakePluginApi([pocsag, aprs], []);
  const store = new PluginStore(fake.api);
  const inst = instance({ plugin_id: "org.superhet.decoder.rtl433" });
  render(<InstanceSettings instance={inst} store={store} />);
  fireEvent.change(screen.getByRole("textbox", { name: "Protocols" }), {
    target: { value: "19, x" },
  });
  await settle();
  expect(fake.api.reconfigure).not.toHaveBeenCalled();
  expect(screen.getByRole("status")).toHaveTextContent("whole numbers");
});

it("says when a decoder has no settings", () => {
  const inst = instance({ plugin_id: APRS_ID });
  const store = new PluginStore(fakePluginApi([aprs], [inst]).api);
  render(<InstanceSettings instance={inst} store={store} />);
  expect(screen.getByText("This decoder has no settings.")).toBeInTheDocument();
});
