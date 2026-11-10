import { Fragment, useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { createDeviceListApi, type DeviceInfo, type DeviceListApi } from "./api";
import { DEVICE_STORAGE_KEY, requestedDevice, resolveDevice, validDeviceId } from "./choice";
import { AuthContext, useAuth } from "../auth/context";
import { readOnlyOnDevice } from "../auth/permissions";
import { DevicesContext } from "./context";

/** Refresh interval of the device list (online/offline). */
export const POLL_MS = 10_000;

interface Props {
  /** The receiver for the chosen device; remounts when the device changes. */
  children: (device: string) => ReactNode;
  api?: DeviceListApi;
}

const defaultApi = createDeviceListApi();

/**
 * Polls the device list and owns the choice of device. The receiver below is keyed by the device,
 * so a switch tears down every spectrum, audio, VFO and plugin subscription of the old device and
 * starts the new one clean. Only an explicit choice is stored; an automatic fallback (the stored
 * device is gone) is not.
 */
export function DevicesProvider({ children, api = defaultApi }: Props) {
  const auth = useAuth();
  const [devices, setDevices] = useState<readonly DeviceInfo[]>([]);
  const [requested, setRequested] = useState<string | null>(() => {
    try {
      return requestedDevice(window.location.search, window.localStorage);
    } catch {
      return null;
    }
  });

  useEffect(() => {
    let live = true;
    let controller = new AbortController();
    const refresh = () => {
      controller.abort();
      controller = new AbortController();
      api
        .list(controller.signal)
        .then((list) => {
          if (live) setDevices(list);
        })
        .catch(() => {
          // Keep the last list: a hiccup must not flip every device offline.
        });
    };
    refresh();
    const timer = window.setInterval(refresh, POLL_MS);
    const onVisible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      controller.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [api]);

  const select = useCallback((id: string) => {
    if (!validDeviceId(id)) return;
    try {
      window.localStorage.setItem(DEVICE_STORAGE_KEY, id);
    } catch {
      // Private mode: the choice just lasts for this page.
    }
    setRequested(id);
  }, []);

  const device = resolveDevice(requested, devices);
  const value = useMemo(() => ({ devices, device, select }), [devices, device, select]);
  // A device the policy only lets this account look at: everything below sees read-only
  // permissions (no tuning, VFOs, lock, profiles or plugins), the server refuses them anyway.
  const readOnly = devices.some((d) => d.id === device && !d.canControl);
  const scoped = useMemo(
    () => (auth && readOnly ? { ...auth, permissions: readOnlyOnDevice(auth.permissions) } : auth),
    [auth, readOnly],
  );
  const body = <Fragment key={device}>{children(device)}</Fragment>;
  return (
    <DevicesContext.Provider value={value}>
      {scoped ? <AuthContext.Provider value={scoped}>{body}</AuthContext.Provider> : body}
    </DevicesContext.Provider>
  );
}
