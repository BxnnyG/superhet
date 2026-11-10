/** Send the whole browser to another site (the identity provider's end-session page). */
export function leaveTo(url: string): void {
  window.location.assign(url);
}
