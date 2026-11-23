import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fakeAdsbApi, onlineSettingsRaw } from "../adsb/testing";
import { OnlineDetailsCard } from "./OnlineDetailsCard";

beforeEach(() => vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "setTimeout"] }));
afterEach(() => vi.useRealTimers());

const flush = () => act(() => vi.advanceTimersByTimeAsync(0));

describe("OnlineDetailsCard", () => {
  it("is off by default, names both sources and what each one gets", async () => {
    const { api } = fakeAdsbApi();
    render(<OnlineDetailsCard api={api} />);
    await flush();
    expect(screen.getByRole("switch", { name: "Online details" })).toHaveAttribute(
      "aria-checked",
      "false",
    );
    const adsbdb = screen.getByTestId("online-source-adsbdb");
    expect(adsbdb).toHaveTextContent("adsbdb.com");
    expect(adsbdb).toHaveTextContent("ICAO address and callsign");
    expect(screen.getByTestId("online-source-planespotters")).toHaveTextContent(
      "Planespotters.net",
    );
    expect(screen.getByTestId("online-details")).toHaveTextContent(
      "The receiver position, accounts and viewers are never sent",
    );
  });

  it("switching on offers both sources; each can be switched off on its own", async () => {
    const { api, state } = fakeAdsbApi();
    render(<OnlineDetailsCard api={api} />);
    await flush();
    await act(async () => {
      fireEvent.click(screen.getByRole("switch", { name: "Online details" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(api.setOnlineSettings).toHaveBeenLastCalledWith({
      enabled: true,
      adsbdb: true,
      planespotters: true,
    });
    expect(state.onlineAdmin.settings.enabled).toBe(true);

    await act(async () => {
      fireEvent.click(
        within(screen.getByTestId("online-source-planespotters")).getByRole("switch"),
      );
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(api.setOnlineSettings).toHaveBeenLastCalledWith({
      enabled: true,
      adsbdb: true,
      planespotters: false,
    });
  });

  it("cannot be edited while the server's environment overrides it", async () => {
    const { api, state } = fakeAdsbApi();
    state.onlineAdmin = onlineSettingsRaw({
      settings: { enabled: true, adsbdb: true, planespotters: true },
      locked: true,
    });
    render(<OnlineDetailsCard api={api} />);
    await flush();
    expect(screen.getByText(/SUPERHET_ADSB_ONLINE/)).toBeInTheDocument();
    for (const toggle of screen.getAllByRole("switch")) expect(toggle).toBeDisabled();
  });

  it("shows a failed save", async () => {
    const { api } = fakeAdsbApi();
    api.setOnlineSettings.mockRejectedValueOnce(new Error("nope"));
    render(<OnlineDetailsCard api={api} />);
    await flush();
    await act(async () => {
      fireEvent.click(screen.getByRole("switch", { name: "Online details" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole("alert")).toBeInTheDocument();
  });
});
