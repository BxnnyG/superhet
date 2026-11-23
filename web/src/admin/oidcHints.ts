/**
 * Live hints for the single sign-on form. The issuer check mirrors the list in
 * `crates/superhet-api/src/auth/oidc/hints.rs` (the server stays authoritative and refuses such
 * an issuer unless it is kept on purpose); nothing here ever changes what the admin typed.
 */

/** [path suffix, what it is, whether removing it leaves the issuer]. */
const ENDPOINTS: ReadonlyArray<readonly [string, string, boolean]> = [
  ["/.well-known/openid-configuration", "discovery document", true],
  ["/.well-known/jwks.json", "key set", true],
  ["/oauth/v2/authorize", "authorization endpoint", true],
  ["/oauth/v2/token", "token endpoint", true],
  ["/oauth/v2/keys", "key set", true],
  ["/oidc/v1/userinfo", "userinfo endpoint", true],
  ["/oidc/v1/end_session", "end session endpoint", true],
  ["/protocol/openid-connect/auth", "authorization endpoint", true],
  ["/protocol/openid-connect/token", "token endpoint", true],
  ["/protocol/openid-connect/userinfo", "userinfo endpoint", true],
  ["/protocol/openid-connect/certs", "key set", true],
  ["/protocol/openid-connect/logout", "end session endpoint", true],
  ["/application/o/authorize", "authorization endpoint", false],
  ["/application/o/token", "token endpoint", false],
  ["/application/o/userinfo", "userinfo endpoint", false],
  ["/api/oidc/token", "token endpoint", true],
  ["/api/oidc/userinfo", "userinfo endpoint", true],
  ["/oauth2/authorize", "authorization endpoint", true],
  ["/oauth2/token", "token endpoint", true],
  ["/authorize", "authorization endpoint", true],
  ["/token", "token endpoint", true],
  ["/userinfo", "userinfo endpoint", true],
  ["/jwks", "key set", true],
  ["/keys", "key set", true],
  ["/certs", "key set", true],
];

export interface IssuerHint {
  /** What looks wrong. */
  message: string;
  /** The likely issuer, if it can be told. */
  suggested: string | null;
}

/** Whether `issuer` ends in the path of a known provider endpoint instead of being an issuer. */
export function issuerHint(issuer: string): IssuerHint | null {
  let url: URL;
  try {
    url = new URL(issuer.trim());
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  const segments = url.pathname.split("/").filter((p) => p !== "");
  const path = `/${segments.join("/")}`;
  let best: (typeof ENDPOINTS)[number] | null = null;
  for (const e of ENDPOINTS) {
    if (path.endsWith(e[0]) && (!best || e[0].length > best[0].length)) best = e;
  }
  if (!best) return null;
  const [suffix, what, strippable] = best;
  const keep = Math.max(0, segments.length - suffix.split("/").filter((p) => p !== "").length);
  const suggested = strippable ? [url.origin, ...segments.slice(0, keep)].join("/") : null;
  return {
    message: `This looks like the provider's ${what} (${path}), not its issuer. The issuer is the base address that serves <issuer>/.well-known/openid-configuration.`,
    suggested,
  };
}

/**
 * A warning when the public URL is not the address this page is open at: the provider sends the
 * browser back to the public URL, so a different host or port lands on a site that has no
 * session and (on another origin) would not even be Superhet.
 */
export function publicUrlMismatch(publicUrl: string, browserOrigin: string): string | null {
  let url: URL;
  try {
    url = new URL(publicUrl.trim());
  } catch {
    return null;
  }
  if (url.origin === browserOrigin) return null;
  return `The public URL (${url.origin}) is not the address this page is open at (${browserOrigin}). After the provider's login the browser is sent to the public URL, so sign-in only works if that is where users reach Superhet.`;
}
