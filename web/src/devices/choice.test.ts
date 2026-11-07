import { expect, it } from "vitest";
import type { DeviceInfo } from "./api";
import { DEVICE_STORAGE_KEY, requestedDevice, resolveDevice, validDeviceId } from "./choice";

const dev = (id: string, online = true): DeviceInfo => ({
  id,
  label: null,
  driver: "rtlsdr",
  tuner: null,
  online,
  centerHz: null,
  sampleRate: null,
});

const storage = (value: string | null) => ({ getItem: () => value });

it("accepts only ids the daemon accepts", () => {
  expect(["dev0", "rtl-1", "a_b=c"].every(validDeviceId)).toBe(true);
  expect(["", "a.b", "a b", "a*", "x".repeat(65), null].some(validDeviceId)).toBe(false);
});

it("the URL wins over the stored choice; junk is ignored", () => {
  expect(requestedDevice("?dev=rtl1", storage("dev0"))).toBe("rtl1");
  expect(requestedDevice("", storage("dev1"))).toBe("dev1");
  expect(requestedDevice("?dev=a.b", storage("dev1"))).toBe("dev1");
  expect(requestedDevice("", storage("a.b"))).toBeNull();
  expect(requestedDevice("", null)).toBeNull();
  expect(DEVICE_STORAGE_KEY).toBe("superhet.device");
});

it("keeps single-device installs on dev0 until the list says otherwise", () => {
  expect(resolveDevice(null, [])).toBe("dev0");
  expect(resolveDevice(null, [dev("dev0")])).toBe("dev0");
  expect(resolveDevice("dev1", [])).toBe("dev1");
});

it("a listed request stays, an unserved one gives way to the first online device", () => {
  expect(resolveDevice("dev1", [dev("dev0"), dev("dev1", false)])).toBe("dev1");
  expect(resolveDevice("gone", [dev("a", false), dev("b")])).toBe("b");
  expect(resolveDevice("gone", [dev("a", false)])).toBe("a");
  // A renamed single device replaces the old dev0 default.
  expect(resolveDevice(null, [dev("rtl0")])).toBe("rtl0");
});
