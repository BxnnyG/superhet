import { createContext, useContext } from "react";
import type { DeviceInfo } from "./api";
import { DEFAULT_DEVICE } from "./choice";

export interface DevicesValue {
  /** Every device the server knows, live or last known; empty until the first answer. */
  devices: readonly DeviceInfo[];
  /** The device the receiver shows. */
  device: string;
  /** Switch the receiver to another device (remembered in this browser). */
  select: (id: string) => void;
}

export const DevicesContext = createContext<DevicesValue>({
  devices: [],
  device: DEFAULT_DEVICE,
  select: () => undefined,
});

export function useDevices(): DevicesValue {
  return useContext(DevicesContext);
}
