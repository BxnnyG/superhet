// Browser side of Web Push: register the service worker, ask for permission (only ever from a
// click), subscribe with the server's VAPID key and hand the subscription to the server.

import type { PushApi } from "./api";

const STORED_ID = "superhet.push.subscription";

export function pushSupported(): boolean {
  return (
    typeof navigator !== "undefined" &&
    "serviceWorker" in navigator &&
    typeof window !== "undefined" &&
    "PushManager" in window &&
    "Notification" in window
  );
}

/** base64url public key to the byte array `pushManager.subscribe` wants. */
export function keyBytes(b64url: string): Uint8Array<ArrayBuffer> {
  const pad = "=".repeat((4 - (b64url.length % 4)) % 4);
  const raw = atob((b64url + pad).replace(/-/g, "+").replace(/_/g, "/"));
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

function b64url(buffer: ArrayBuffer | null): string {
  if (!buffer) return "";
  let s = "";
  for (const b of new Uint8Array(buffer)) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The id of the server-side subscription this browser created, if any. */
export function localSubscriptionId(): number | null {
  const v = Number(localStorage.getItem(STORED_ID));
  return Number.isInteger(v) && v > 0 ? v : null;
}

/**
 * Turn notifications on in this browser. Must be called from a click handler: the permission
 * prompt is shown here and nowhere else.
 */
export async function enablePush(api: PushApi, label: string): Promise<number> {
  if (!pushSupported()) throw new Error("This browser does not support push notifications.");
  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error("Notifications are blocked for this site. Allow them in the browser settings.");
  }
  const registration = await navigator.serviceWorker.register("/sw.js");
  await navigator.serviceWorker.ready;
  const { publicKey } = await api.get();
  const sub =
    (await registration.pushManager.getSubscription()) ??
    (await registration.pushManager.subscribe({
      userVisibleOnly: true,
      applicationServerKey: keyBytes(publicKey),
    }));
  const id = await api.subscribe({
    endpoint: sub.endpoint,
    keys: { p256dh: b64url(sub.getKey("p256dh")), auth: b64url(sub.getKey("auth")) },
    label,
  });
  localStorage.setItem(STORED_ID, String(id));
  return id;
}

/** Drop the browser's own subscription (after the server one was removed). */
export async function forgetLocalPush(): Promise<void> {
  localStorage.removeItem(STORED_ID);
  if (!pushSupported()) return;
  const registration = await navigator.serviceWorker.getRegistration("/sw.js");
  const sub = await registration?.pushManager.getSubscription();
  await sub?.unsubscribe();
}
