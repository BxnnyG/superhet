// Device discovery (GET /api/v1/devices, docs/api-devices.md) with runtime validation.

import { apiFetch } from "../auth/session";
import { ApiError, httpError, isRec, readJson } from "../spectrum/api";

/** One device daemon as the list reports it. */
export interface DeviceInfo {
  id: string;
  /** Operator-chosen name (`SUPERHET_DEVICE_LABEL`), if any. */
  label: string | null;
  driver: string | null;
  tuner: string | null;
  /** True while the daemon answers; false for a last-known entry. */
  online: boolean;
  centerHz: number | null;
  sampleRate: number | null;
}

export interface DeviceListApi {
  list(signal?: AbortSignal): Promise<DeviceInfo[]>;
}

const str = (v: unknown): string | null => (typeof v === "string" && v !== "" ? v : null);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);

export function parseDeviceInfo(raw: unknown): DeviceInfo {
  if (!isRec(raw) || typeof raw.id !== "string" || raw.id === "") {
    throw new ApiError("invalid device list entry");
  }
  return {
    id: raw.id,
    label: str(raw.label),
    driver: str(raw.driver),
    tuner: str(raw.tuner),
    online: raw.online === true,
    centerHz: num(raw.center_hz),
    sampleRate: num(raw.sample_rate),
  };
}

export function createDeviceListApi(fetchImpl: typeof fetch = apiFetch): DeviceListApi {
  return {
    async list(signal) {
      let res: Response;
      try {
        res = await fetchImpl("/api/v1/devices", { signal });
      } catch (e) {
        throw new ApiError(e instanceof Error ? e.message : "network error");
      }
      const body = await readJson(res);
      if (!res.ok) throw httpError(res, body);
      const list = isRec(body) ? body.devices : undefined;
      if (!Array.isArray(list)) throw new ApiError("invalid device list");
      return list.map(parseDeviceInfo);
    },
  };
}
