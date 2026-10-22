// The device state document as pushed by /api/v1/ws/state and returned in control replies
// (docs/api-devices.md, "State object"): device settings plus the VFO list.

import { ApiError, isRec, parseDeviceState, type DeviceState } from "../spectrum/api";
import { parseVfoList, type Vfo } from "../vfo/api";

export interface StateDoc {
  device: DeviceState;
  vfos: Vfo[];
}

export function parseStateDoc(raw: unknown): StateDoc {
  if (!isRec(raw)) throw new ApiError("invalid state: not an object");
  return { device: parseDeviceState(raw), vfos: parseVfoList(raw.vfos ?? []) };
}

/**
 * Parse one text frame of the state socket. Returns null for anything that is not a valid
 * `{"v":1,"type":"state","state":{...}}` frame; the feed ignores such frames.
 */
export function parseStateFrame(text: string): StateDoc | null {
  try {
    const msg: unknown = JSON.parse(text);
    if (!isRec(msg) || msg.type !== "state") return null;
    return parseStateDoc(msg.state);
  } catch {
    return null;
  }
}
