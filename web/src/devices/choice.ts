// Which device the browser shows: `?dev=` in the URL, else the last explicit choice (kept in
// localStorage), else the default of single-device installs. Pure helpers; see DevicesProvider.

import type { DeviceInfo } from "./api";

/** Device id of single-device installs (`SUPERHET_DEVICE_ID` default). */
export const DEFAULT_DEVICE = "dev0";
export const DEVICE_STORAGE_KEY = "superhet.device";

/** Same shape the daemon accepts: one subject token, no dots or wildcards. */
const ID_RE = /^[A-Za-z0-9_=-]{1,64}$/;

export function validDeviceId(v: unknown): v is string {
  return typeof v === "string" && ID_RE.test(v);
}

/** The device asked for by the URL or the stored choice; null when neither names a valid one. */
export function requestedDevice(
  search: string,
  storage: Pick<Storage, "getItem"> | null,
): string | null {
  const fromUrl = new URLSearchParams(search).get("dev");
  if (validDeviceId(fromUrl)) return fromUrl;
  try {
    const stored = storage?.getItem(DEVICE_STORAGE_KEY) ?? null;
    return validDeviceId(stored) ? stored : null;
  } catch {
    return null;
  }
}

/**
 * The device to show once the list is known. A requested device that is listed stays; one that
 * nobody serves any more gives way to the first online device (else the first listed). With no
 * list yet (or an empty one) the request or the default stands.
 */
export function resolveDevice(requested: string | null, devices: readonly DeviceInfo[]): string {
  const wanted = requested ?? DEFAULT_DEVICE;
  if (devices.length === 0 || devices.some((d) => d.id === wanted)) return wanted;
  return (devices.find((d) => d.online) ?? devices[0])?.id ?? wanted;
}
