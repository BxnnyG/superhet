import { useCallback, useState } from "react";
import { ROLE_LABEL } from "../auth/permissions";
import { HttpError, describeHttpError } from "../http";
import { Alert, Badge, Button, Select, TextField, Toggle, toasts } from "../ui";
import { type AdminApi, type OidcSettings, type OidcTestResult } from "./api";
import { Card, LoadState, SaveBar, SectionHeader, copyText } from "./common";
import { issuerHint, publicUrlMismatch } from "./oidcHints";
import { useDraft } from "./useDraft";
import { useLoad } from "./useLoad";

const DEFAULT_ROLE_OPTIONS = [
  { value: "", label: "Refuse sign-in" },
  { value: "listener", label: ROLE_LABEL.listener },
  { value: "operator", label: ROLE_LABEL.operator },
  { value: "admin", label: ROLE_LABEL.admin },
];

/** `a, b ,c` to `["a", "b", "c"]`. */
export function parseList(text: string): string[] {
  return text
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s !== "");
}

const isLoopback = (host: string): boolean =>
  host === "localhost" || host === "[::1]" || /^127(\.\d{1,3}){3}$/.test(host);

function urlProblem(label: string, raw: string): string | null {
  let url: URL;
  try {
    url = new URL(raw.trim());
  } catch {
    return `${label} is not a valid URL.`;
  }
  if (url.protocol !== "https:" && !(url.protocol === "http:" && isLoopback(url.hostname))) {
    return `${label} must be an https:// URL (plain http only for localhost).`;
  }
  if (url.username || url.password) return `${label} must not contain credentials.`;
  return null;
}

/** What is wrong with the settings before they are sent (the server validates again). */
export function oidcProblem(s: OidcSettings): string | null {
  if (s.name.trim() === "" || s.name.trim().length > 40)
    return "The button label needs 1-40 characters.";
  if (!s.enabled) return null;
  const issuer = urlProblem("The issuer", s.issuer);
  if (issuer) return issuer;
  if (s.clientId.trim() === "") return "Enter the client ID registered at the provider.";
  const pub = urlProblem("The public URL", s.publicUrl);
  if (pub) return pub;
  if (
    s.defaultRole === "" &&
    s.adminGroups.length + s.operatorGroups.length + s.listenerGroups.length === 0
  ) {
    return "Map at least one group to a role or choose a default role, otherwise nobody can sign in.";
  }
  return null;
}

/** A URL derived from the public URL, to register at the provider, with a copy button. */
function UrlLine({
  label,
  url,
  copyLabel,
}: {
  label: string;
  url: string | null;
  copyLabel: string;
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-xs font-medium text-fg-muted">{label}</span>
      <div className="flex items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md bg-surface-2 px-3 py-2 text-xs">
          {url ?? "Set the public URL first."}
        </code>
        {url && (
          <Button
            size="sm"
            onClick={() => void copyText(url, copyLabel)}
            aria-label={`Copy the ${copyLabel.toLowerCase()}`}
          >
            Copy
          </Button>
        )}
      </div>
    </div>
  );
}

/** "This is probably what you meant" with a button that puts it into the form (not saved). */
function IssuerSuggestion({
  message,
  suggested,
  onUse,
}: {
  message: string;
  suggested: string | null;
  onUse: (issuer: string) => void;
}) {
  return (
    <Alert tone="warn">
      <p>{message}</p>
      {suggested && (
        <p className="mt-2 flex flex-wrap items-center gap-2">
          <code className="rounded bg-surface-2 px-2 py-1 text-xs text-fg">{suggested}</code>
          <Button size="sm" onClick={() => onUse(suggested)}>
            Use this issuer
          </Button>
        </p>
      )}
    </Alert>
  );
}

/** Which discovery request failed and how, for the admin who has to find out why. */
function describeFailure(f: {
  url: string | null;
  status: number | null;
  message: string | null;
}): string {
  const parts = [];
  if (f.status !== null) parts.push(`HTTP ${f.status}`);
  if (f.url) parts.push(f.url);
  return `${parts.join(" from ")}${parts.length ? ": " : ""}${f.message ?? "unknown error"}`;
}

function ListField({
  label,
  value,
  onChange,
  hint,
}: {
  label: string;
  value: string[];
  onChange: (v: string[]) => void;
  hint?: string;
}) {
  const [text, setText] = useState(value.join(", "));
  const parsed = parseList(text);
  // An outside change (Discard, reload) wins over what is typed.
  if (parsed.length !== value.length || parsed.some((v, i) => v !== value[i])) {
    setText(value.join(", "));
  }
  return (
    <TextField
      label={label}
      hint={hint}
      spellCheck={false}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        onChange(parseList(e.target.value));
      }}
    />
  );
}

export function OidcSection({ api }: { api: AdminApi }) {
  const load = useCallback((s: AbortSignal) => api.getOidc(s), [api]);
  const loaded = useLoad(load);
  const [saved, setSaved] = useState<typeof loaded.data>(null);
  const view = saved ?? loaded.data;
  const draft = useDraft(view?.settings ?? null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<OidcTestResult | null>(null);
  const [testing, setTesting] = useState(false);
  /** The server refused an issuer that looks like an endpoint; saving it needs a second click. */
  const [refusedIssuer, setRefusedIssuer] = useState(false);
  /** Discovery result of the last save of an enabled configuration. */
  const [afterSave, setAfterSave] = useState<OidcTestResult | null>(null);
  const s = draft.value;
  const readOnly = view?.source === "env";
  const problem = s ? oidcProblem(s) : null;
  const endpointHint = s ? issuerHint(s.issuer) : null;
  const originMismatch = s ? publicUrlMismatch(s.publicUrl, window.location.origin) : null;

  const update = (patch: Partial<OidcSettings>) => {
    if (s) {
      draft.set({ ...s, ...patch });
      setTest(null);
      setAfterSave(null);
      setRefusedIssuer(false);
      setError(null);
    }
  };

  const save = async (keepIssuer = false) => {
    if (!s || problem) return;
    setBusy(true);
    setError(null);
    setRefusedIssuer(false);
    try {
      const view = await api.putOidc(s, { keepIssuer });
      setSaved(view);
      setAfterSave(view.discovery);
      toasts.push({ tone: "success", title: "Single sign-on settings saved" });
    } catch (e) {
      setError(describeHttpError(e));
      setRefusedIssuer(e instanceof HttpError && e.code === "issuer_is_endpoint");
    } finally {
      setBusy(false);
    }
  };

  const runTest = async () => {
    setTesting(true);
    setTest(null);
    try {
      setTest(await api.testOidc());
    } catch (e) {
      setTest({
        ok: false,
        issuer: null,
        error: describeHttpError(e),
        endSession: false,
        status: null,
        url: null,
        hint: null,
        suggestedIssuer: null,
      });
    } finally {
      setTesting(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Single sign-on"
        description="Let people sign in with an OpenID Connect provider such as Authentik, Keycloak, Pocket ID or Google, next to local accounts."
      />
      <LoadState loading={loaded.loading} error={loaded.error} onRetry={loaded.reload} />
      {s && view && (
        <div className="flex flex-col gap-6">
          {readOnly && (
            <Alert tone="info">
              This configuration comes from the SUPERHET_OIDC_* environment variables and cannot be
              changed here. Remove SUPERHET_OIDC_ISSUER to manage it on this page.
            </Alert>
          )}
          <fieldset
            disabled={readOnly || busy}
            className="m-0 flex min-w-0 flex-col gap-6 border-0 p-0"
          >
            <Card title="Provider">
              <div className="flex flex-col gap-4 p-4">
                <Toggle
                  label="Offer single sign-on on the login page"
                  checked={s.enabled}
                  onChange={(enabled) => update({ enabled })}
                />
                <div className="grid gap-4 sm:grid-cols-2">
                  <TextField
                    label="Button label"
                    hint="Shown as “Sign in with …”."
                    maxLength={40}
                    value={s.name}
                    onChange={(e) => update({ name: e.target.value })}
                  />
                  <TextField
                    label="Client ID"
                    spellCheck={false}
                    value={s.clientId}
                    onChange={(e) => update({ clientId: e.target.value })}
                  />
                </div>
                <TextField
                  label="Issuer URL"
                  placeholder="https://auth.example.org/application/o/superhet/"
                  hint="The provider's discovery document is read from <issuer>/.well-known/openid-configuration."
                  spellCheck={false}
                  value={s.issuer}
                  onChange={(e) => update({ issuer: e.target.value })}
                />
                {endpointHint && (
                  <IssuerSuggestion
                    message={endpointHint.message}
                    suggested={endpointHint.suggested}
                    onUse={(issuer) => update({ issuer })}
                  />
                )}
                <TextField
                  label="Public URL of this instance"
                  placeholder="https://sdr.example.org"
                  hint="Where users reach Superhet in their browser; the redirect URI is derived from it."
                  spellCheck={false}
                  value={s.publicUrl}
                  onChange={(e) => update({ publicUrl: e.target.value })}
                />
                {originMismatch && <Alert tone="warn">{originMismatch}</Alert>}
                <UrlLine
                  label="Callback URL (redirect URI) to register at the provider"
                  url={view.redirectUrl}
                  copyLabel="Callback URL"
                />
                <p className="text-xs text-fg-subtle">
                  Client secret:{" "}
                  {view.clientSecretSet ? (
                    <Badge tone="success">set</Badge>
                  ) : (
                    <Badge tone="neutral">not set</Badge>
                  )}{" "}
                  The secret is never stored in the database or shown here. Provide it to the api
                  service as SUPERHET_OIDC_CLIENT_SECRET (or SUPERHET_OIDC_CLIENT_SECRET_FILE).
                  Providers that offer a public client with PKCE do not need one.
                </p>
              </div>
            </Card>

            <Card
              title="Roles"
              description="Groups are read from the ID token. A user gets the highest role any of their groups grants."
            >
              <div className="flex flex-col gap-4 p-4">
                <div className="grid gap-4 sm:grid-cols-2">
                  <TextField
                    label="Groups claim"
                    hint="Dotted for nested claims, e.g. realm_access.roles."
                    spellCheck={false}
                    value={s.groupsClaim}
                    onChange={(e) => update({ groupsClaim: e.target.value })}
                  />
                  <ListField
                    label="Scopes"
                    hint="Besides openid. Add groups if your provider needs it."
                    value={s.scopes}
                    onChange={(scopes) => update({ scopes })}
                  />
                </div>
                <ListField
                  label="Admin groups"
                  hint="Comma separated."
                  value={s.adminGroups}
                  onChange={(adminGroups) => update({ adminGroups })}
                />
                <ListField
                  label="Operator groups"
                  value={s.operatorGroups}
                  onChange={(operatorGroups) => update({ operatorGroups })}
                />
                <ListField
                  label="Listener groups"
                  value={s.listenerGroups}
                  onChange={(listenerGroups) => update({ listenerGroups })}
                />
                <Select
                  label="Role for everybody else"
                  value={s.defaultRole}
                  onChange={(v) => update({ defaultRole: v as OidcSettings["defaultRole"] })}
                  options={DEFAULT_ROLE_OPTIONS}
                />
              </div>
            </Card>

            <Card title="Accounts">
              <div className="flex flex-col gap-4 p-4">
                <Toggle
                  label="Create an account on first sign-in"
                  checked={s.autoProvision}
                  onChange={(autoProvision) => update({ autoProvision })}
                />
                <p className="-mt-2 text-xs text-fg-subtle">
                  Off: only people who already have an account (or are linked below) can sign in.
                </p>
                <Toggle
                  label="Link to an existing account by verified email address"
                  checked={s.linkByEmail}
                  onChange={(linkByEmail) => update({ linkByEmail })}
                />
                <p className="-mt-2 text-xs text-fg-subtle">
                  The first sign-in is linked to the local account with the same email address, but
                  only if the provider marks the address as verified. Set addresses on the Users
                  page. Only enable this if you trust the provider to verify addresses.
                </p>
                <Toggle
                  label="Roles follow the provider's groups at every sign-in"
                  checked={s.syncRoles}
                  onChange={(syncRoles) => update({ syncRoles })}
                />
                <p className="-mt-2 text-xs text-fg-subtle">
                  Off: the role is set when the account is created and changed only here. The last
                  active admin is never demoted.
                </p>
                <TextField
                  label="Username claim"
                  hint="Proposed login name for new accounts."
                  spellCheck={false}
                  value={s.usernameClaim}
                  onChange={(e) => update({ usernameClaim: e.target.value })}
                />
              </div>
            </Card>

            <Card title="Sign-out">
              <div className="flex flex-col gap-3 p-4">
                <Toggle
                  label="Signing out of Superhet also ends the provider session"
                  checked={s.rpLogout}
                  onChange={(rpLogout) => update({ rpLogout })}
                />
                <p className="-mt-2 text-xs text-fg-subtle">
                  Only for sessions that came from single sign-on, and only if the provider
                  advertises an end session endpoint. Register the post-logout redirect URI below at
                  the provider.
                </p>
                <UrlLine
                  label="Post-logout redirect URI to register at the provider"
                  url={view.postLogoutUrl}
                  copyLabel="Post-logout redirect URI"
                />
                <Toggle
                  label="Accept back-channel logout from the provider"
                  checked={s.backchannelLogout}
                  onChange={(backchannelLogout) => update({ backchannelLogout })}
                />
                <p className="-mt-2 text-xs text-fg-subtle">
                  When the user signs out at the provider (or an admin ends their session there),
                  the matching Superhet sessions end too. Register the URL below as the client's
                  back-channel logout URL; the provider must be able to reach it.
                </p>
                <UrlLine
                  label="Back-channel logout URL to register at the provider"
                  url={view.backchannelUrl}
                  copyLabel="Back-channel logout URL"
                />
              </div>
            </Card>

            <Card title="Password sign-in">
              <div className="flex flex-col gap-3 p-4">
                <Toggle
                  label="Allow signing in with a password"
                  checked={s.localLogin}
                  disabled={s.localLogin && !view.oidcAdminExists}
                  onChange={(localLogin) => update({ localLogin })}
                />
                <p className="text-xs text-fg-subtle">
                  {view.oidcAdminExists
                    ? "At least one enabled admin can sign in through single sign-on, so password sign-in may be switched off. If no such admin remains, the password form comes back on its own."
                    : "Password sign-in can be switched off only after an admin account has signed in through single sign-on (or was linked to it)."}
                  {view.keepLocalLogin &&
                    " SUPERHET_OIDC_KEEP_LOCAL_LOGIN is set: password sign-in stays available."}
                </p>
              </div>
            </Card>
          </fieldset>

          {view.lastError && (
            <Alert tone="warn">
              <p className="font-medium">
                The last sign-in attempt could not reach the provider (
                {new Date(view.lastError.at * 1000).toLocaleString()}).
              </p>
              <p className="mt-1">{describeFailure(view.lastError)}</p>
              <p className="mt-1 text-xs">
                People only see “Single sign-on is not available right now.” This note is kept until
                the next successful attempt or a change of these settings.
              </p>
            </Alert>
          )}
          {afterSave && !afterSave.ok && (
            <Alert tone="warn">
              <p className="font-medium">
                Saved, but the provider could not be used. Signing in with single sign-on will fail
                until this is fixed.
              </p>
              <p className="mt-1">{describeFailure({ ...afterSave, message: afterSave.error })}</p>
            </Alert>
          )}
          {refusedIssuer && (
            <Alert tone="warn">
              <p>
                The issuer was not saved because it looks like an endpoint URL. If it really is the
                issuer of your provider, save it as entered.
              </p>
              <p className="mt-2">
                <Button size="sm" onClick={() => void save(true)} disabled={busy}>
                  Save as entered
                </Button>
              </p>
            </Alert>
          )}
          {problem && <Alert tone="warn">{problem}</Alert>}
          {(test?.suggestedIssuer || (afterSave && !afterSave.ok && afterSave.suggestedIssuer)) && (
            <IssuerSuggestion
              message="The provider publishes its discovery document under a different issuer on the same host."
              suggested={test?.suggestedIssuer ?? afterSave?.suggestedIssuer ?? null}
              onUse={(issuer) => update({ issuer })}
            />
          )}
          {test && (
            <Alert tone={test.ok ? "info" : "danger"}>
              {test.ok
                ? `The provider answered. Issuer: ${test.issuer ?? ""}. ${test.endSession ? "It supports ending its session on sign-out." : "It does not advertise an end session endpoint, so signing out here leaves the provider session alone."}`
                : `The provider could not be used: ${describeFailure({ ...test, message: test.error })}`}
            </Alert>
          )}
          <Card className="sticky bottom-4 z-10 shadow-pop">
            <div className="flex flex-col">
              <div className="flex items-center gap-2 border-b border-border px-4 py-3">
                <Button onClick={() => void runTest()} disabled={testing || draft.dirty}>
                  {testing ? "Testing…" : "Test connection"}
                </Button>
                <span className="text-xs text-fg-subtle">
                  {draft.dirty ? "Save first, the test uses the saved settings." : ""}
                </span>
              </div>
              {!readOnly && (
                <SaveBar
                  dirty={draft.dirty}
                  busy={busy}
                  error={error}
                  onSave={() => void save()}
                  onDiscard={() => {
                    draft.discard();
                    setError(null);
                  }}
                />
              )}
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
