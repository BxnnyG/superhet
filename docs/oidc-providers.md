# OIDC provider recipes (Authentik, Keycloak)

Hand-run setup notes for the single sign-on in `docs/api-oidc.md` (ADR-0019, logout: ADR-0024).

**Not verified here.** Superhet's automated tests use an in-process mock provider only; nobody has run the steps below against a real Authentik or Keycloak for this repository. They are written from the providers' documentation as of late 2026, menu names change between versions, and the logout parts depend on the provider version. Treat a recipe as a checklist, run the check at the end, and send a correction if something differs. Zitadel was the original target (ROADMAP P5) and is not covered here.

Common to both: set `SUPERHET_PUBLIC_URL` (or the public URL field) to the address users type in their browser, e.g. `https://sdr.example.org`. Superhet then shows the three URLs to register on the admin page (Settings, "Single sign-on"):

- Redirect URI: `https://sdr.example.org/api/v1/auth/oidc/callback`
- Post-logout redirect URI: `https://sdr.example.org/`
- Back-channel logout URL: `https://sdr.example.org/api/v1/auth/oidc/backchannel-logout` (the provider calls this from its own network; it must be able to reach it)

The client secret goes to the api service as `SUPERHET_OIDC_CLIENT_SECRET` (or `..._FILE`).

## Authentik

1. Applications, Create with Provider: choose "OAuth2/OpenID Provider". Client type confidential. Note client ID and secret.
2. Redirect URIs/Origins: strict match on the redirect URI above.
3. **Signing Key: pick a certificate** (e.g. "authentik Self-signed Certificate"). Without one authentik signs ID tokens with HS256 (using the client secret), which Superhet refuses; the login fails with `token_invalid`.
4. Scopes: `openid`, `email`, `profile`. The default `profile` scope mapping carries the user's group names in the `groups` claim, which is Superhet's default groups claim. Create groups (e.g. `superhet-admins`) and map them in Superhet.
5. Logout: the discovery document advertises `end_session_endpoint` (`.../application/o/<slug>/end-session/`). Recent versions have a field for post-logout redirect URIs (enter the URI above); on versions without it the user stays on authentik's logged-out page. For back-channel logout, recent versions offer a logout URI with method "back-channel" on the OAuth2 provider; enter the back-channel URL there. If your version has no such setting, only RP-initiated logout is available and the "Accept back-channel logout" switch in Superhet can stay on (it just never gets called).
6. Superhet: issuer `https://authentik.example.org/application/o/<slug>/` (copy it from the provider page, **including the trailing slash**), client ID, scopes `email profile` (default), admin groups as needed. "Test connection" should report "supports ending its session on sign-out".

## Keycloak

1. Realm (not `master`), Clients, Create client: type OpenID Connect, client ID `superhet`. Capability config: Client authentication on (confidential), Standard flow on, everything else off.
2. Valid redirect URIs: the redirect URI above. Valid post logout redirect URIs: the post-logout URI above. Web origins can stay empty (Superhet does the code exchange on the server).
3. Advanced / Logout settings: Backchannel logout URL = the back-channel URL above; "Backchannel logout session required" on (so the logout token carries `sid`).
4. Credentials tab: copy the client secret.
5. Groups: Clients, superhet, Client scopes, `superhet-dedicated`, Add mapper, By configuration, "Group Membership": token claim name `groups`, "Full group path" off, add to ID token on. Or map realm roles with a "User Realm Role" mapper and set Superhet's groups claim to the claim name you chose (nested names work: `realm_access.roles`).
6. Superhet: issuer `https://keycloak.example.org/realms/<realm>` (older Keycloak versions: `/auth/realms/<realm>`), client ID, scopes `email profile`. Keycloak over plain http works only with `SUPERHET_OIDC_ALLOW_HTTP=true` on a lab setup.

## Check by hand (either provider)

1. Admin page: "Test connection" answers ok and names the issuer. Enable, save.
2. Sign in with the provider. In the audit log: `auth.oidc_login` for your account.
3. Sign out of Superhet: the browser should visit the provider's logout page and land on the Superhet login page (or on the provider's "logged out" page when the provider ignores `post_logout_redirect_uri`). "Sign in with ..." must now ask for credentials again. Audit: `auth.logout` with `provider_logout: true`.
4. Sign in again, then sign out **at the provider** (its account page, or end the session in its admin UI). Within a moment the Superhet tab's next request must fail with 401 and show the login page; audit: `auth.oidc_backchannel_logout` with `sessions: 1`. If nothing happens: the provider cannot reach the URL (look for a refused connection in its logs), or Superhet logged a refusal (`outcome: denied`, `reason`), see `docs/api-audit.md`.
5. A password-signed-in admin session in another browser must survive step 4.
