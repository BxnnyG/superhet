import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useEffect } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createDeviceListApi, parseDeviceInfo, type DeviceInfo, type DeviceListApi } from "./api";
import { AuthContext, usePermissions } from "../auth/context";
import { accountStatus, authValue } from "../auth/testing";
import { DEVICE_STORAGE_KEY } from "./choice";
import { useDevices } from "./context";
import { DevicesProvider, POLL_MS } from "./DevicesProvider";

const dev = (id: string, online = true, label: string | null = null): DeviceInfo => ({
  id,
  label,
  driver: "file",
  tuner: null,
  online,
  centerHz: 100e6,
  sampleRate: 2_048_000,
  canControl: true,
});

function Probe() {
  const { device, devices, select } = useDevices();
  return (
    <div>
      <span data-testid="device">{device}</span>
      <span data-testid="count">{devices.length}</span>
      <button onClick={() => select("dev1")}>pick dev1</button>
    </div>
  );
}

beforeEach(() => window.localStorage.removeItem(DEVICE_STORAGE_KEY));
afterEach(() => vi.useRealTimers());

it("starts on dev0, lists the devices and switches on an explicit choice", async () => {
  const api: DeviceListApi = { list: vi.fn().mockResolvedValue([dev("dev0"), dev("dev1")]) };
  render(<DevicesProvider api={api}>{() => <Probe />}</DevicesProvider>);
  expect(screen.getByTestId("device")).toHaveTextContent("dev0");
  await waitFor(() => expect(screen.getByTestId("count")).toHaveTextContent("2"));
  fireEvent.click(screen.getByRole("button", { name: "pick dev1" }));
  expect(screen.getByTestId("device")).toHaveTextContent("dev1");
  expect(window.localStorage.getItem(DEVICE_STORAGE_KEY)).toBe("dev1");
});

it("remounts the receiver when the device changes", async () => {
  let mounts = 0;
  function Mark() {
    useEffect(() => {
      mounts += 1;
    }, []);
    return <Probe />;
  }
  const api: DeviceListApi = { list: vi.fn().mockResolvedValue([dev("dev0"), dev("dev1")]) };
  render(<DevicesProvider api={api}>{() => <Mark />}</DevicesProvider>);
  await waitFor(() => expect(screen.getByTestId("count")).toHaveTextContent("2"));
  expect(mounts).toBeGreaterThan(0);
  const before = mounts;
  fireEvent.click(screen.getByRole("button", { name: "pick dev1" }));
  expect(mounts).toBe(before + 1);
});

it("falls back to the only listed device without storing it", async () => {
  const api: DeviceListApi = { list: vi.fn().mockResolvedValue([dev("rtl0")]) };
  render(<DevicesProvider api={api}>{() => <Probe />}</DevicesProvider>);
  await waitFor(() => expect(screen.getByTestId("device")).toHaveTextContent("rtl0"));
  expect(window.localStorage.getItem(DEVICE_STORAGE_KEY)).toBeNull();
});

it("polls, and keeps the last list when a poll fails", async () => {
  vi.useFakeTimers();
  const list = vi
    .fn()
    .mockResolvedValueOnce([dev("dev0"), dev("dev1")])
    .mockRejectedValue(new Error("down"));
  render(<DevicesProvider api={{ list }}>{() => <Probe />}</DevicesProvider>);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(0);
  });
  expect(screen.getByTestId("count")).toHaveTextContent("2");
  await act(async () => {
    await vi.advanceTimersByTimeAsync(POLL_MS);
  });
  expect(list).toHaveBeenCalledTimes(2);
  expect(screen.getByTestId("count")).toHaveTextContent("2");
});

it("gives a device the account may only look at read-only permissions", async () => {
  function Perms() {
    const p = usePermissions();
    return (
      <span data-testid="perms">
        {String(p.canTune)}/{String(p.canUsePlugins)}/{String(p.deviceReadOnly)}
      </span>
    );
  }
  const api: DeviceListApi = {
    list: vi.fn().mockResolvedValue([dev("dev0"), { ...dev("dev1"), canControl: false }]),
  };
  render(
    <AuthContext.Provider value={authValue(accountStatus("operator"))}>
      <DevicesProvider api={api}>
        {() => (
          <>
            <Probe />
            <Perms />
          </>
        )}
      </DevicesProvider>
    </AuthContext.Provider>,
  );
  await waitFor(() => expect(screen.getByTestId("count")).toHaveTextContent("2"));
  expect(screen.getByTestId("perms")).toHaveTextContent("true/true/false");
  fireEvent.click(screen.getByRole("button", { name: "pick dev1" }));
  expect(screen.getByTestId("perms")).toHaveTextContent("false/false/true");
});

it("parses the list reply and rejects malformed entries", async () => {
  const body = {
    v: 1,
    devices: [{ id: "dev1", label: "Roof", online: true, center_hz: 1, sample_rate: 2 }],
  };
  const ok = vi.fn().mockResolvedValue(new Response(JSON.stringify(body)));
  const [d] = await createDeviceListApi(ok).list();
  expect(d).toMatchObject({ id: "dev1", label: "Roof", online: true, centerHz: 1, sampleRate: 2 });
  expect(d?.canControl).toBe(true);
  // The policy flag: only an explicit `control: false` makes a device read-only.
  expect(parseDeviceInfo({ id: "a", access: { view: true, control: false } }).canControl).toBe(
    false,
  );
  expect(parseDeviceInfo({ id: "a", access: { view: true, control: true } }).canControl).toBe(true);
  expect(() => parseDeviceInfo({ online: true })).toThrow();
  const bad = vi.fn().mockResolvedValue(new Response(JSON.stringify({ devices: 3 })));
  await expect(createDeviceListApi(bad).list()).rejects.toThrow();
  const down = vi
    .fn()
    .mockResolvedValue(new Response(JSON.stringify({ error: "bus" }), { status: 503 }));
  await expect(createDeviceListApi(down).list()).rejects.toThrow();
});
