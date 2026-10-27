import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AuthContext } from "../auth/context";
import { accountStatus, authValue } from "../auth/testing";
import { fake, vfo } from "../shell/fixtures";
import { VfoTabs } from "../shell/VfoTabs";
import { VfoDecoderMark, worstState } from "./parts";
import { PluginStore } from "./store";
import { fakePluginApi, instance, pocsag, rtl433 } from "./testing";

describe("worstState", () => {
  it("picks the most urgent state", () => {
    expect(worstState([instance({ state: "running" }), instance({ state: "error" })])).toBe(
      "error",
    );
    expect(worstState([instance({ state: "running" }), instance({ state: "backoff" })])).toBe(
      "backoff",
    );
    expect(worstState([instance({ state: "running" })])).toBe("running");
    expect(worstState([])).toBe("stopped");
  });
});

describe("VfoDecoderMark", () => {
  it("renders nothing without decoders", () => {
    const { container } = render(<VfoDecoderMark instances={[]} names={[]} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("names the decoders and their state", () => {
    render(
      <VfoDecoderMark
        instances={[instance(), instance({ id: "i-2", state: "error" })]}
        names={["POCSAG", "APRS"]}
      />,
    );
    expect(screen.getByRole("img")).toHaveAttribute(
      "aria-label",
      "Decoders: POCSAG running, APRS failed",
    );
  });
});

describe("VFO tabs", () => {
  it("show a state mark on the VFO a decoder runs on", async () => {
    const { r } = fake([vfo(1), vfo(2)], 1);
    const { api } = fakePluginApi([pocsag, rtl433], [instance({ vfo: 2 })]);
    const store = new PluginStore(api);
    (r as unknown as { plugins: PluginStore }).plugins = store;
    store.start();
    await act(() => new Promise<void>((res) => setTimeout(res, 0)));
    render(
      <AuthContext value={authValue(accountStatus("operator", "olga", 3))}>
        <VfoTabs r={r} />
      </AuthContext>,
    );
    const marks = screen.getAllByRole("img", { name: /^Decoders:/ });
    expect(marks).toHaveLength(1);
    expect(marks[0]).toHaveAttribute("aria-label", "Decoders: POCSAG / FLEX pagers running");
    // It sits in the tab of VFO 2.
    expect(marks[0]?.closest("li")).toHaveTextContent("2");
    expect(marks[0]?.closest("li")?.querySelector("[aria-pressed]")).toHaveAttribute(
      "aria-label",
      expect.stringContaining("VFO 2"),
    );
    store.stop();
  });
});
